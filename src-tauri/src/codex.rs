use crate::config::{AppConfig, DEFAULT_CONTEXT_WINDOW};
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
};
use toml_edit::{value, DocumentMut, Item, Table};

const APP_SERVER_WRAPPER_JS: &str = r#"const { spawn } = require("node:child_process");
const readline = require("node:readline");

const realCodex = process.env.OPENCODEX_REAL_CODEX_PATH;
if (!realCodex) {
  console.error("OPENCODEX_REAL_CODEX_PATH is not set");
  process.exit(1);
}

const args = process.argv.slice(2);
const child = spawn(realCodex, args, { stdio: ["pipe", "pipe", "inherit"] });
child.stdout.pipe(process.stdout);

if (args.includes("app-server")) {
  const input = readline.createInterface({ input: process.stdin, crlfDelay: Infinity });
  input.on("line", (line) => {
    try {
      const message = JSON.parse(line);
      if (message.method === "thread/list" && message.params?.modelProviders == null) {
        message.params = { ...message.params, modelProviders: [] };
        line = JSON.stringify(message);
      }
    } catch {
      // Forward non-JSON input unchanged.
    }
    child.stdin.write(`${line}\n`);
  });
  input.on("close", () => child.stdin.end());
} else {
  process.stdin.pipe(child.stdin);
}

let shuttingDown = false;
for (const signal of ["SIGINT", "SIGTERM", "SIGHUP"]) {
  process.on(signal, () => {
    if (shuttingDown) return;
    shuttingDown = true;
    child.kill(signal);
    setTimeout(() => {
      child.kill("SIGKILL");
      process.exit(0);
    }, 1000).unref();
  });
}
child.on("exit", (code, signal) => {
  process.exit(code ?? (signal ? 1 : 0));
});
"#;

const APP_SERVER_WRAPPER_SH: &str = r#"#!/bin/sh
exec "$OPENCODEX_NODE_PATH" "$OPENCODEX_WRAPPER_JS_PATH" "$@"
"#;

#[derive(Clone)]
pub struct AppServerWrapper {
    launcher: PathBuf,
    script: PathBuf,
}

pub fn prepare_app_server_wrapper(data_dir: &Path) -> Result<AppServerWrapper, String> {
    fs::create_dir_all(data_dir).map_err(|e| format!("创建 Codex wrapper 目录失败: {e}"))?;
    let launcher = data_dir.join("codex-app-server-wrapper.sh");
    let script = data_dir.join("codex-app-server-wrapper.cjs");
    fs::write(&script, APP_SERVER_WRAPPER_JS)
        .map_err(|e| format!("写入 Codex wrapper 脚本失败: {e}"))?;
    fs::write(&launcher, APP_SERVER_WRAPPER_SH)
        .map_err(|e| format!("写入 Codex wrapper 启动器失败: {e}"))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&launcher, fs::Permissions::from_mode(0o700))
            .map_err(|e| format!("设置 Codex wrapper 权限失败: {e}"))?;
    }
    Ok(AppServerWrapper { launcher, script })
}

pub fn default_config_path() -> Result<PathBuf, String> {
    Ok(home_dir()?.join(".codex").join("config.toml"))
}

fn home_dir() -> Result<PathBuf, String> {
    let home = std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .ok_or_else(|| "无法确定用户主目录".to_string())?;
    Ok(PathBuf::from(home))
}

pub fn model_catalog_path() -> Result<PathBuf, String> {
    Ok(home_dir()?
        .join(".codex")
        .join("model-catalogs")
        .join("opencodex.json"))
}

pub fn is_injected(path: &Path) -> bool {
    fs::read_to_string(path)
        .ok()
        .and_then(|s| s.parse::<DocumentMut>().ok())
        .and_then(|d| {
            d.get("model_providers")
                .and_then(|x| x.get("opencodex_proxy"))
                .cloned()
        })
        .is_some()
}

pub fn inject(config_path: &Path, backup_path: &Path, config: &AppConfig) -> Result<(), String> {
    if let Some(parent) = config_path.parent() {
        fs::create_dir_all(parent).map_err(|e| format!("创建 Codex 配置目录失败: {e}"))?;
    }
    let original = fs::read_to_string(config_path).unwrap_or_default();
    // Always refresh the safety backup from a non-injected config so ChatGPT
    // rewrites do not leave us restoring a half-injected file later.
    if !is_injected(config_path) || !backup_path.exists() {
        if !is_injected(config_path) {
            if let Some(parent) = backup_path.parent() {
                fs::create_dir_all(parent).map_err(|e| format!("创建备份目录失败: {e}"))?;
            }
            fs::write(backup_path, original.as_bytes())
                .map_err(|e| format!("备份 Codex 配置失败: {e}"))?;
        } else if !backup_path.exists() {
            backup(config_path, backup_path)?;
        }
    }
    let mut doc = original
        .parse::<DocumentMut>()
        .map_err(|e| format!("现有 Codex config.toml 无法解析，未做修改: {e}"))?;

    // Route Codex through the local proxy while keeping the rest of the file
    // (plugins, projects, marketplaces) intact. Desktop currently hides the
    // custom model picker when this flag is false, even though app-server can
    // load the local catalog. Keep the auth context flag enabled for Desktop's
    // picker/plugins, while the local proxy still ignores Codex's inbound token
    // and uses each configured provider API key for the actual model request.
    doc["model_provider"] = value("opencodex_proxy");
    doc["model_catalog_json"] = value("~/.codex/model-catalogs/opencodex.json");
    let mut provider = Table::new();
    provider["name"] = value("OnlyCodex");
    provider["base_url"] = value(format!(
        "http://{}:{}/v1",
        config.settings.bind_address, config.settings.port
    ));
    provider["wire_api"] = value("responses");
    provider["requires_openai_auth"] = value(true);
    if !doc.as_table().contains_key("model_providers") {
        doc["model_providers"] = Item::Table(Table::new());
    }
    doc["model_providers"]["opencodex_proxy"] = Item::Table(provider);

    // Select the first advertised alias so every Codex request has an explicit route.
    if let Some(first) = config.mappings.iter().find(|mapping| {
        mapping.enabled
            && config
                .providers
                .iter()
                .any(|provider| provider.id == mapping.provider_id && provider.enabled)
    }) {
        doc["model"] = value(&first.codex_model);
    } else if let Some((slug, _)) = config
        .providers
        .iter()
        .filter(|p| p.enabled)
        .flat_map(|p| {
            p.models
                .iter()
                .filter(|m| m.enabled)
                .map(move |m| (m.id.as_str(), p))
        })
        .next()
    {
        let current = doc
            .get("model")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let known: Vec<String> = config
            .providers
            .iter()
            .filter(|p| p.enabled)
            .flat_map(|p| p.models.iter().filter(|m| m.enabled).map(|m| m.id.clone()))
            .collect();
        if current.is_empty() || !known.iter().any(|id| id == &current) {
            doc["model"] = value(slug);
        }
    }
    atomic_write(config_path, doc.to_string().as_bytes())
}

pub fn backup(config_path: &Path, backup_path: &Path) -> Result<(), String> {
    if backup_path.exists() {
        return Ok(());
    }
    if let Some(parent) = backup_path.parent() {
        fs::create_dir_all(parent).map_err(|e| format!("创建备份目录失败: {e}"))?;
    }
    let original = fs::read(config_path).unwrap_or_default();
    fs::write(backup_path, original).map_err(|e| format!("备份 Codex 配置失败: {e}"))
}

pub fn update_model_catalog(config: &AppConfig) -> Result<usize, String> {
    update_model_catalog_at(config, &model_catalog_path()?)
}

fn update_model_catalog_at(config: &AppConfig, path: &Path) -> Result<usize, String> {
    let template: Value = serde_json::from_str(include_str!("../resources/opencodex-catalog.json"))
        .map_err(|e| format!("内置模型目录损坏: {e}"))?;
    let template_models = template
        .get("models")
        .and_then(Value::as_array)
        .ok_or("内置模型目录缺少 models")?;
    let by_slug: BTreeMap<&str, &Value> = template_models
        .iter()
        .filter_map(|m| m.get("slug").and_then(Value::as_str).map(|slug| (slug, m)))
        .collect();
    let base = template_models.first().cloned().ok_or("内置模型目录为空")?;

    let enabled_mappings: Vec<_> = config
        .mappings
        .iter()
        .filter(|m| {
            m.enabled
                && config
                    .providers
                    .iter()
                    .any(|p| p.id == m.provider_id && p.enabled)
        })
        .collect();
    let mut desired: BTreeMap<String, (String, u64)> = BTreeMap::new();
    // Codex only sees explicit aliases. Raw provider model ids remain available
    // as mapping targets but are not advertised as native Codex models.
    for mapping in enabled_mappings {
        let provider = config
            .providers
            .iter()
            .find(|p| p.id == mapping.provider_id);
        let display = provider
            .map(|p| format!("{}:{}", p.name, mapping.upstream_model))
            .unwrap_or_else(|| mapping.upstream_model.clone());
        let context = provider
            .and_then(|p| p.models.iter().find(|m| m.id == mapping.upstream_model))
            .map(|m| m.context_window)
            .unwrap_or(DEFAULT_CONTEXT_WINDOW);
        desired.insert(mapping.codex_model.clone(), (display, context));
    }

    let models: Vec<Value> = desired
        .into_iter()
        .map(|(slug, (display, context))| {
            let mut entry = by_slug
                .get(slug.as_str())
                .map(|v| (*v).clone())
                .unwrap_or_else(|| base.clone());
            if let Some(object) = entry.as_object_mut() {
                object.insert("slug".into(), json!(slug));
                object.insert("display_name".into(), json!(display));
                object.insert("description".into(), json!("OnlyCodex mapped model"));
                object.insert("supported_in_api".into(), json!(true));
                object.insert("visibility".into(), json!("list"));
                object.insert("context_window".into(), json!(context));
                object.insert("max_context_window".into(), json!(context));
            }
            entry
        })
        .collect();
    let count = models.len();
    let mut catalog = template;
    catalog["models"] = Value::Array(models);
    catalog["fetched_at"] = json!(chrono::Utc::now().to_rfc3339());
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| format!("创建模型目录失败: {e}"))?;
    }
    let raw = serde_json::to_vec_pretty(&catalog).map_err(|e| e.to_string())?;
    atomic_write(path, &raw)?;
    Ok(count)
}

pub fn restore(config_path: &Path, backup_path: &Path) -> Result<(), String> {
    if !backup_path.exists() {
        if !is_injected(config_path) {
            return Ok(());
        }
        return Err("找不到安全备份，拒绝覆盖当前 Codex 配置".into());
    }
    let backup = fs::read(backup_path).map_err(|e| format!("读取备份失败: {e}"))?;
    atomic_write(config_path, &backup)?;
    fs::remove_file(backup_path).map_err(|e| format!("配置已恢复，但清理备份失败: {e}"))
}

fn atomic_write(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let temp = path.with_extension("opencodex.tmp");
    fs::write(&temp, bytes).map_err(|e| format!("写入临时文件失败: {e}"))?;
    fs::rename(&temp, path).map_err(|e| format!("替换配置文件失败: {e}"))
}

/// macOS Codex Desktop ships as ChatGPT.app (bundle id com.openai.codex).
#[cfg(target_os = "macos")]
const MACOS_CODEX_APPS: &[&str] = &["ChatGPT", "Codex"];

/// Ask Codex/ChatGPT to quit normally so login and conversation state can flush to disk.
pub fn quit_app() -> Result<(), String> {
    #[cfg(target_os = "macos")]
    {
        for name in MACOS_CODEX_APPS {
            let _ = Command::new("osascript")
                .args(["-e", &format!("tell application \"{name}\" to quit")])
                .status();
        }
        for _ in 0..50 {
            if !macos_codex_is_running() {
                std::thread::sleep(std::time::Duration::from_millis(400));
                return Ok(());
            }
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
        Err("Codex 未能正常退出；为保护登录状态和历史记录，已取消强制终止，请手动退出后重试".into())
    }
    #[cfg(target_os = "windows")]
    {
        let _ = Command::new("taskkill").args(["/IM", "Codex.exe"]).status();
        let _ = Command::new("taskkill")
            .args(["/IM", "ChatGPT.exe"])
            .status();
        std::thread::sleep(std::time::Duration::from_millis(400));
        Ok(())
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    Ok(())
}

#[cfg(target_os = "macos")]
fn macos_codex_is_running() -> bool {
    MACOS_CODEX_APPS.iter().any(|name| {
        Command::new("pgrep")
            .args(["-x", name])
            .status()
            .map(|status| status.success())
            .unwrap_or(false)
    })
}

pub fn launch_app(wrapper: Option<&AppServerWrapper>) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    {
        // ChatGPT.app is the current Codex Desktop; fall back to "Codex" if present.
        let mut last_err = String::new();
        for name in MACOS_CODEX_APPS {
            let app_path = PathBuf::from("/Applications").join(format!("{name}.app"));
            if !app_path.exists() {
                continue;
            }
            let mut command = Command::new("open");
            if let Some(wrapper) = wrapper {
                let resources = app_path.join("Contents").join("Resources");
                let real_codex = resources.join("codex");
                let node = resources.join("cua_node").join("bin").join("node");
                command
                    .arg("--env")
                    .arg(format!("CODEX_CLI_PATH={}", wrapper.launcher.display()))
                    .arg("--env")
                    .arg(format!(
                        "OPENCODEX_WRAPPER_JS_PATH={}",
                        wrapper.script.display()
                    ))
                    .arg("--env")
                    .arg(format!(
                        "OPENCODEX_REAL_CODEX_PATH={}",
                        real_codex.display()
                    ))
                    .arg("--env")
                    .arg(format!("OPENCODEX_NODE_PATH={}", node.display()));
            }
            match command.args(["-a", name]).status() {
                Ok(status) if status.success() => return Ok(()),
                Ok(status) => last_err = format!("{name}: open {status}"),
                Err(e) => last_err = format!("{name}: {e}"),
            }
        }
        // Last resort cannot install the wrapper because the app path is unknown.
        match Command::new("open")
            .args(["-b", "com.openai.codex"])
            .status()
        {
            Ok(status) if status.success() && wrapper.is_none() => Ok(()),
            Ok(_) if wrapper.is_some() => Err(format!(
                "重启 Codex 失败：无法定位应用路径以安装历史记录兼容 wrapper；{last_err}"
            )),
            Ok(status) => Err(format!("重启 Codex 失败: open {status}; {last_err}")),
            Err(e) => Err(format!(
                "重启 Codex 失败（已尝试 ChatGPT/Codex/bundle id）: {last_err}; {e}"
            )),
        }
    }
    #[cfg(target_os = "windows")]
    {
        // Match upstream opencodex: Windows package family name for Codex.
        let shell = Command::new("cmd")
            .args([
                "/C",
                "start",
                "",
                "shell:AppsFolder\\OpenAI.Codex_2p2nqsd0c76g0!App",
            ])
            .spawn();
        if shell.is_ok() {
            return Ok(());
        }
        Command::new("cmd")
            .args(["/C", "start", "", "Codex"])
            .spawn()
            .map_err(|e| format!("重启 Codex 失败: {e}"))?;
        Ok(())
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        Err("当前平台尚未实现 Codex 自动重启".into())
    }
}

pub fn restart_app(wrapper: Option<&AppServerWrapper>) -> Result<(), String> {
    quit_app()?;
    launch_app(wrapper)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn injection_and_restore_are_lossless() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        let backup = dir.path().join("backup.toml");
        let original = "model = \"old-model\"\n\n[projects.\"/tmp\"]\ntrust_level = \"trusted\"\n\n[features]\napps = true\n";
        fs::write(&path, original).unwrap();
        inject(&path, &backup, &AppConfig::default()).unwrap();
        assert!(is_injected(&path));
        let injected = fs::read_to_string(&path).unwrap();
        assert!(injected.contains("model_catalog_json"));
        assert!(injected.contains("requires_openai_auth = true"));
        assert!(!injected.contains("experimental_bearer_token"));
        assert!(injected.contains("trust_level = \"trusted\""));
        assert!(injected.contains("apps = true"));
        assert_eq!(fs::read_to_string(&backup).unwrap(), original);
        restore(&path, &backup).unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), original);
    }

    #[test]
    fn app_server_wrapper_preserves_all_thread_providers() {
        let dir = tempfile::tempdir().unwrap();
        let wrapper = prepare_app_server_wrapper(dir.path()).unwrap();
        assert!(wrapper.launcher.exists());
        let script = fs::read_to_string(wrapper.script).unwrap();
        assert!(script.contains("message.method === \"thread/list\""));
        assert!(script.contains("modelProviders: []"));
        assert!(script.contains("child.kill(\"SIGKILL\")"));
    }

    #[test]
    fn catalog_contains_only_mapped_model_names() {
        use crate::config::{Mapping, ModelConfig, Protocol, Provider};
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("opencodex.json");
        let mut config = AppConfig::default();
        config.providers.push(Provider {
            id: "p1".into(),
            name: "Local".into(),
            base_url: "http://127.0.0.1:11434/v1".into(),
            api_key: String::new(),
            protocol: Protocol::ChatCompletions,
            enabled: true,
            models: vec![
                ModelConfig {
                    id: "qwen".into(),
                    name: "Qwen".into(),
                    enabled: true,
                    context_window: 32_000,
                },
                ModelConfig {
                    id: "llama".into(),
                    name: "Llama".into(),
                    enabled: true,
                    context_window: 64_000,
                },
            ],
        });
        config.mappings.push(Mapping {
            id: "m1".into(),
            codex_model: "codex-alias".into(),
            provider_id: "p1".into(),
            upstream_model: "qwen".into(),
            enabled: true,
        });

        assert_eq!(update_model_catalog_at(&config, &path).unwrap(), 1);
        let catalog: Value = serde_json::from_str(&fs::read_to_string(path).unwrap()).unwrap();
        let models = catalog["models"].as_array().unwrap();
        assert_eq!(models.len(), 1);
        assert_eq!(models[0]["slug"], "codex-alias");
        assert!(models[0].get("base_instructions").is_some());
        assert_eq!(models[0]["context_window"], 32_000);
    }
}

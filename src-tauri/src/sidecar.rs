use crate::{config::AppConfig, AppRuntime};
use serde_json::{json, Map, Value};
use std::{
    collections::VecDeque,
    io::{BufRead, BufReader},
    net::TcpListener,
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::Arc,
};

pub struct ServerHandle {
    child: Child,
    stopped: bool,
    pub started_at: i64,
}

impl ServerHandle {
    pub fn stop(&mut self) {
        if self.stopped {
            return;
        }
        self.stopped = true;
        #[cfg(unix)]
        {
            let process_group = self.child.id() as i32;
            // PyInstaller one-file executables spawn a child process. Signal the
            // whole dedicated group so that child cannot survive as an orphan.
            unsafe {
                libc::kill(-process_group, libc::SIGTERM);
            }
            std::thread::sleep(std::time::Duration::from_millis(400));
            unsafe {
                libc::kill(-process_group, libc::SIGKILL);
            }
        }
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

impl Drop for ServerHandle {
    fn drop(&mut self) {
        self.stop();
    }
}

pub async fn spawn(runtime: Arc<AppRuntime>, config: &AppConfig) -> Result<ServerHandle, String> {
    ensure_address_available(&config.settings.bind_address, config.settings.port)?;
    let proxy_config = runtime.data_dir.join("proxy-config.json");
    write_proxy_config(&proxy_config, config)?;
    let (program, prefix) = locate_program()?;
    let mut command = Command::new(&program);
    // Do NOT pass empty --chat-base-url "": argparse treats the next flag as its
    // value and silently drops --config. Desktop builds leave the default empty
    // via CLI default / env, and only use --config for providers.
    command
        .args(prefix)
        .args([
            "--bind",
            &config.settings.bind_address,
            "--port",
            &config.settings.port.to_string(),
            "--timeout-sec",
            &config.settings.request_timeout_seconds.to_string(),
            "--max-body-mb",
            &config.settings.max_body_mb.to_string(),
            "--default-protocol",
            "responses",
            "--config",
            &proxy_config.display().to_string(),
        ])
        .env("OPENCODEX_CONFIG", proxy_config.as_os_str())
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if let Ok(catalog) = crate::codex::model_catalog_path() {
        command.env("CODEX_MODEL_CATALOG", catalog);
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x08000000);
    }
    let mut child = command
        .spawn()
        .map_err(|e| format!("无法启动 OnlyCodex sidecar {}: {e}", program.display()))?;
    let mut stderr_thread = child.stderr.take().map(|stderr| {
        let rt = runtime.clone();
        std::thread::spawn(move || {
            let mut recent = VecDeque::with_capacity(8);
            for line in BufReader::new(stderr).lines().map_while(Result::ok) {
                rt.log_upstream(line.clone());
                if recent.len() == 8 {
                    recent.pop_front();
                }
                recent.push_back(line);
            }
            recent
        })
    });
    if let Some(stdout) = child.stdout.take() {
        let rt = runtime.clone();
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                rt.log_upstream(line);
            }
        });
    }
    let mut handle = ServerHandle {
        child,
        stopped: false,
        started_at: chrono::Utc::now().timestamp(),
    };
    let health = health_url(&config.settings.bind_address, config.settings.port);
    for _ in 0..40 {
        if let Some(status) = handle.child.try_wait().map_err(|e| e.to_string())? {
            let detail = stderr_thread
                .take()
                .and_then(|thread| thread.join().ok())
                .and_then(|lines| startup_error_detail(&lines));
            return Err(match detail {
                Some(detail) => format!("OnlyCodex sidecar 提前退出: {status}；{detail}"),
                None => format!("OnlyCodex sidecar 提前退出: {status}"),
            });
        }
        if runtime
            .client
            .get(&health)
            .send()
            .await
            .map(|r| r.status().is_success())
            .unwrap_or(false)
        {
            return Ok(handle);
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    handle.stop();
    Err("OnlyCodex sidecar 启动超时".into())
}

fn ensure_address_available(bind_address: &str, port: u16) -> Result<(), String> {
    TcpListener::bind((bind_address, port))
        .map(drop)
        .map_err(|error| format!("无法启动代理：{bind_address}:{port} 端口不可用（{error}）"))
}

fn health_url(bind_address: &str, port: u16) -> String {
    if bind_address.contains(':') {
        format!("http://[{bind_address}]:{port}/health")
    } else {
        format!("http://{bind_address}:{port}/health")
    }
}

fn startup_error_detail(lines: &VecDeque<String>) -> Option<String> {
    lines
        .iter()
        .rev()
        .find(|line| line.contains("Error:") || line.contains("ERROR") || line.contains("OSError"))
        .or_else(|| lines.back())
        .map(|line| line.trim().to_string())
        .filter(|line| !line.is_empty())
}

fn locate_program() -> Result<(PathBuf, Vec<String>), String> {
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let dir = exe.parent().ok_or("无法确定应用程序目录")?;
    for name in packaged_names() {
        let candidate = dir.join(name);
        if candidate.exists() {
            return Ok((candidate, vec![]));
        }
    }
    #[cfg(debug_assertions)]
    {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .ok_or("无法确定项目目录")?
            .to_path_buf();
        let entry = root.join("python").join("sidecar_main.py");
        let bundled = PathBuf::from("/Users/baomeng/.cache/codex-runtimes/codex-primary-runtime/dependencies/python/bin/python3");
        let python = if bundled.exists() {
            bundled
        } else {
            PathBuf::from("python3")
        };
        return Ok((python, vec![entry.display().to_string()]));
    }
    #[allow(unreachable_code)]
    Err("找不到已打包的 OnlyCodex sidecar".into())
}

fn packaged_names() -> Vec<&'static str> {
    #[cfg(target_os = "windows")]
    {
        return vec![
            "opencodex-sidecar.exe",
            "opencodex-sidecar-x86_64-pc-windows-msvc.exe",
        ];
    }
    #[cfg(target_os = "macos")]
    {
        return vec![
            "opencodex-sidecar",
            "opencodex-sidecar-aarch64-apple-darwin",
            "opencodex-sidecar-x86_64-apple-darwin",
        ];
    }
    #[cfg(target_os = "linux")]
    {
        return vec![
            "opencodex-sidecar",
            "opencodex-sidecar-x86_64-unknown-linux-gnu",
        ];
    }
}

pub fn write_proxy_config(path: &Path, config: &AppConfig) -> Result<(), String> {
    let mut providers = Map::new();
    for provider in config.providers.iter().filter(|p| p.enabled) {
        providers.insert(provider.id.clone(), json!({
            "baseUrl": provider.base_url,
            "apiKey": if provider.api_key.is_empty() { "not-required" } else { &provider.api_key },
            "protocol": match &provider.protocol {
                crate::config::Protocol::Responses => "responses",
                crate::config::Protocol::ChatCompletions => "chat_completions",
            },
            "models": provider.models.iter().filter(|m| m.enabled).map(|m| m.id.clone()).collect::<Vec<_>>()
        }));
    }
    let mappings: Map<String, Value> = config
        .mappings
        .iter()
        .filter(|m| {
            m.enabled
                && config
                    .providers
                    .iter()
                    .any(|p| p.id == m.provider_id && p.enabled)
        })
        .map(|m| {
            (
                m.codex_model.clone(),
                Value::String(format!("{}:{}", m.provider_id, m.upstream_model)),
            )
        })
        .collect();
    let content = serde_json::to_string_pretty(&json!({"providers":providers,"mappings":mappings}))
        .map_err(|e| e.to_string())?;
    std::fs::write(path, content).map_err(|e| format!("写入 sidecar 配置失败: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exports_upstream_compatible_config() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("proxy.json");
        write_proxy_config(&path, &AppConfig::default()).unwrap();
        let value: Value = serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
        assert!(value["providers"].is_object());
        assert!(value["mappings"].is_object());
    }

    #[test]
    fn exports_responses_as_the_default_provider_protocol() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("proxy.json");
        let mut config = AppConfig::default();
        config.providers.push(crate::config::Provider {
            id: "native".into(),
            name: "Native Responses".into(),
            base_url: "https://example.com/v1".into(),
            api_key: "key".into(),
            protocol: crate::config::Protocol::Responses,
            enabled: true,
            models: vec![],
        });
        write_proxy_config(&path, &config).unwrap();
        let value: Value = serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
        assert_eq!(value["providers"]["native"]["protocol"], "responses");
    }

    #[test]
    fn reports_an_unavailable_port_before_spawning() {
        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let port = listener.local_addr().unwrap().port();
        let error = ensure_address_available("127.0.0.1", port).unwrap_err();
        assert!(error.contains(&port.to_string()));
        assert!(error.contains("端口不可用"));
    }

    #[test]
    fn formats_ipv6_health_url() {
        assert_eq!(health_url("::1", 8787), "http://[::1]:8787/health");
    }
}

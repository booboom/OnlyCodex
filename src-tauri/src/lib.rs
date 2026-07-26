mod codex;
mod config;
mod sidecar;

use config::{AppConfig, ModelConfig, DEFAULT_CONTEXT_WINDOW};
use serde::Serialize;
use sidecar::ServerHandle;
use std::{
    collections::VecDeque,
    path::PathBuf,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc,
    },
};
use tauri::{
    menu::{MenuBuilder, MenuItemBuilder},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
    Manager, State, WindowEvent,
};
use tokio::sync::{Mutex, RwLock};

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct LogEntry {
    id: u64,
    timestamp: f64,
    level: String,
    message: String,
}

pub struct AppRuntime {
    config: RwLock<AppConfig>,
    config_file: PathBuf,
    data_dir: PathBuf,
    server: Mutex<Option<ServerHandle>>,
    logs: RwLock<VecDeque<LogEntry>>,
    log_sequence: AtomicU64,
    requests: AtomicU64,
    errors: AtomicU64,
    client: reqwest::Client,
}

impl AppRuntime {
    fn log(&self, level: &str, message: String) {
        let entry = LogEntry {
            id: self.log_sequence.fetch_add(1, Ordering::Relaxed),
            timestamp: chrono::Utc::now().timestamp_millis() as f64 / 1000.0,
            level: level.into(),
            message,
        };
        if level == "ERROR" {
            self.errors.fetch_add(1, Ordering::Relaxed);
        }
        if let Ok(mut logs) = self.logs.try_write() {
            if logs.len() >= 1000 {
                logs.pop_front();
            }
            logs.push_back(entry);
        }
    }
    pub fn info(&self, message: String) {
        self.log("INFO", message)
    }
    pub fn error(&self, message: String) {
        self.log("ERROR", message)
    }
    pub fn log_upstream(&self, line: String) {
        let value = serde_json::from_str::<serde_json::Value>(&line).ok();
        let event = value
            .as_ref()
            .and_then(|v| v.get("event"))
            .and_then(|v| v.as_str())
            .map(str::to_string);
        if event.as_deref() == Some("request.received") {
            self.requests.fetch_add(1, Ordering::Relaxed);
        }
        let level = if event
            .as_deref()
            .is_some_and(|e| e.contains("error") || e.contains("failed") || e.contains("crashed"))
        {
            "ERROR"
        } else {
            "INFO"
        };
        let message = if let Some(v) = value {
            let name = event.as_deref().unwrap_or("upstream");
            let model = v
                .get("model")
                .and_then(|x| x.as_str())
                .map(|x| format!(" · {x}"))
                .unwrap_or_default();
            let detail = v
                .get("message")
                .and_then(|x| x.as_str())
                .map(|x| format!(" · {x}"))
                .unwrap_or_default();
            format!("{name}{model}{detail}")
        } else {
            line
        };
        self.log(level, message);
    }
    fn backup_path(&self) -> PathBuf {
        self.data_dir.join("codex-config.backup.toml")
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ProxyStatus {
    running: bool,
    address: String,
    started_at: Option<i64>,
    request_count: u64,
    error_count: u64,
    config_injected: bool,
    config_path: String,
    backup_path: Option<String>,
}

async fn status(runtime: &AppRuntime) -> Result<ProxyStatus, String> {
    let cfg = runtime.config.read().await;
    let server = runtime.server.lock().await;
    let path = codex::default_config_path()?;
    let backup = runtime.backup_path();
    Ok(ProxyStatus {
        running: server.is_some(),
        address: format!("{}:{}", cfg.settings.bind_address, cfg.settings.port),
        started_at: server.as_ref().map(|s| s.started_at),
        request_count: runtime.requests.load(Ordering::Relaxed),
        error_count: runtime.errors.load(Ordering::Relaxed),
        config_injected: codex::is_injected(&path),
        config_path: path.display().to_string(),
        backup_path: backup.exists().then(|| backup.display().to_string()),
    })
}

#[tauri::command]
async fn get_config(runtime: State<'_, Arc<AppRuntime>>) -> Result<AppConfig, String> {
    Ok(runtime.config.read().await.clone())
}
#[tauri::command]
async fn save_config(
    runtime: State<'_, Arc<AppRuntime>>,
    config: AppConfig,
) -> Result<AppConfig, String> {
    if runtime.server.lock().await.is_some() {
        return Err("请先停止代理再修改配置，避免运行中的路由与界面不一致".into());
    }
    config::save(&runtime.config_file, &config)?;
    *runtime.config.write().await = config.clone();
    runtime.info("应用配置已保存".into());
    Ok(config)
}
#[tauri::command]
async fn get_status(runtime: State<'_, Arc<AppRuntime>>) -> Result<ProxyStatus, String> {
    status(&runtime).await
}

async fn start_proxy_inner(runtime: &Arc<AppRuntime>) -> Result<ProxyStatus, String> {
    if runtime.server.lock().await.is_some() {
        return status(runtime).await;
    }
    let cfg = runtime.config.read().await.clone();
    let has_mapping = cfg.mappings.iter().any(|mapping| {
        mapping.enabled
            && cfg
                .providers
                .iter()
                .any(|provider| provider.id == mapping.provider_id && provider.enabled)
    });
    if !has_mapping {
        return Err("请至少配置并启用一条指向已启用供应商的模型映射".into());
    }
    let catalog_count = codex::update_model_catalog(&cfg)?;
    let app_server_wrapper = cfg
        .settings
        .restart_codex_on_change
        .then(|| codex::prepare_app_server_wrapper(&runtime.data_dir))
        .transpose()?;
    // Quit Codex first so it can flush conversations and cannot race the config injection.
    if cfg.settings.restart_codex_on_change {
        codex::quit_app()?;
    }
    let path = codex::default_config_path()?;
    codex::inject(&path, &runtime.backup_path(), &cfg)?;
    // Verify injection stuck (ChatGPT may race-write if still alive).
    if !codex::is_injected(&path) {
        return Err(
            "配置注入未生效：Codex/ChatGPT 可能仍在运行并覆盖了 config.toml，请先完全退出后重试"
                .into(),
        );
    }
    let handle = match sidecar::spawn(runtime.clone(), &cfg).await {
        Ok(h) => h,
        Err(e) => {
            let _ = codex::restore(&path, &runtime.backup_path());
            return Err(e);
        }
    };
    *runtime.server.lock().await = Some(handle);
    runtime.info(format!(
        "代理已启动于 {}:{}，模型目录包含 {} 个模型",
        cfg.settings.bind_address, cfg.settings.port, catalog_count
    ));
    if cfg.settings.restart_codex_on_change {
        if let Err(e) = codex::launch_app(app_server_wrapper.as_ref()) {
            runtime.log("WARN", e);
        } else {
            runtime.info("已重启 Codex（保留登录、历史、记忆与插件配置；模型请求直达代理）".into());
        }
    }
    status(runtime).await
}

#[tauri::command]
async fn start_proxy(runtime: State<'_, Arc<AppRuntime>>) -> Result<ProxyStatus, String> {
    start_proxy_inner(runtime.inner()).await
}

#[tauri::command]
async fn stop_proxy(runtime: State<'_, Arc<AppRuntime>>) -> Result<ProxyStatus, String> {
    codex::quit_app()?;
    if let Some(mut handle) = runtime.server.lock().await.take() {
        handle.stop();
    }
    let path = codex::default_config_path()?;
    codex::restore(&path, &runtime.backup_path())?;
    runtime.info("代理已停止，Codex 已关闭，原始配置已恢复".into());
    status(&runtime).await
}

#[tauri::command]
async fn restore_codex_config(runtime: State<'_, Arc<AppRuntime>>) -> Result<ProxyStatus, String> {
    if runtime.server.lock().await.is_some() {
        return Err("请先停止代理再手动恢复配置".into());
    }
    codex::restore(&codex::default_config_path()?, &runtime.backup_path())?;
    runtime.info("已从安全备份恢复 Codex 配置".into());
    status(&runtime).await
}

#[tauri::command]
async fn backup_codex_config(runtime: State<'_, Arc<AppRuntime>>) -> Result<ProxyStatus, String> {
    codex::backup(&codex::default_config_path()?, &runtime.backup_path())?;
    runtime.info("已手动备份 Codex 配置".into());
    status(&runtime).await
}

#[tauri::command]
async fn refresh_model_catalog(runtime: State<'_, Arc<AppRuntime>>) -> Result<usize, String> {
    let config = runtime.config.read().await;
    let count = codex::update_model_catalog(&config)?;
    runtime.info(format!("模型目录已刷新，共 {count} 个模型"));
    Ok(count)
}

#[tauri::command]
async fn restart_codex(runtime: State<'_, Arc<AppRuntime>>) -> Result<(), String> {
    let wrapper = if runtime.server.lock().await.is_some() {
        Some(codex::prepare_app_server_wrapper(&runtime.data_dir)?)
    } else {
        None
    };
    codex::restart_app(wrapper.as_ref())?;
    runtime.info("Codex 已重启".into());
    Ok(())
}

#[tauri::command]
async fn test_provider(
    runtime: State<'_, Arc<AppRuntime>>,
    provider_id: String,
) -> Result<Vec<ModelConfig>, String> {
    let cfg = runtime.config.read().await;
    let p = cfg
        .providers
        .iter()
        .find(|p| p.id == provider_id)
        .cloned()
        .ok_or("供应商不存在")?;
    let timeout = cfg.settings.request_timeout_seconds;
    drop(cfg);
    let base = p.base_url.trim_end_matches('/');
    let url = if base.ends_with("/models") {
        base.to_string()
    } else {
        format!("{base}/models")
    };
    let mut req = runtime
        .client
        .get(url)
        .timeout(std::time::Duration::from_secs(timeout));
    if !p.api_key.is_empty() {
        req = req.bearer_auth(&p.api_key);
    }
    let response = req.send().await.map_err(|e| format!("连接失败: {e}"))?;
    let status_code = response.status();
    if !status_code.is_success() {
        return Err(format!(
            "供应商返回 {}: {}",
            status_code,
            response.text().await.unwrap_or_default()
        ));
    }
    let data: serde_json::Value = response
        .json()
        .await
        .map_err(|e| format!("模型列表不是有效 JSON: {e}"))?;
    let models = data
        .get("data")
        .and_then(|v| v.as_array())
        .or_else(|| data.get("models").and_then(|v| v.as_array()))
        .ok_or("响应中没有 data 或 models 数组")?;
    let mut discovered: Vec<ModelConfig> = models
        .iter()
        .filter_map(|m| {
            let id = m
                .as_str()
                .or_else(|| m.get("id").and_then(|v| v.as_str()))
                .or_else(|| m.get("name").and_then(|v| v.as_str()))?;
            let name = m
                .get("display_name")
                .or_else(|| m.get("name"))
                .and_then(|v| v.as_str())
                .unwrap_or(id);
            let context_window = m
                .get("context_length")
                .or_else(|| m.get("context_window"))
                .and_then(|v| v.as_u64())
                .unwrap_or(DEFAULT_CONTEXT_WINDOW);
            Some(ModelConfig {
                id: id.into(),
                name: name.into(),
                enabled: true,
                context_window,
            })
        })
        .collect();
    discovered.sort_by(|a, b| a.id.cmp(&b.id));
    discovered.dedup_by(|a, b| a.id == b.id);
    runtime.info(format!(
        "{} 连接成功，发现 {} 个模型",
        p.name,
        discovered.len()
    ));
    Ok(discovered)
}

#[tauri::command]
async fn get_logs(runtime: State<'_, Arc<AppRuntime>>) -> Result<Vec<LogEntry>, String> {
    Ok(runtime.logs.read().await.iter().cloned().collect())
}
#[tauri::command]
async fn clear_logs(runtime: State<'_, Arc<AppRuntime>>) -> Result<(), String> {
    runtime.logs.write().await.clear();
    Ok(())
}
#[tauri::command]
fn open_codex_config() -> Result<(), String> {
    let path = codex::default_config_path()?;
    open_path(&path)
}
#[tauri::command]
fn reveal_data_dir(runtime: State<'_, Arc<AppRuntime>>) -> Result<(), String> {
    open_path(&runtime.data_dir)
}
fn open_path(path: &std::path::Path) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    {
        std::process::Command::new("open")
            .arg(path)
            .spawn()
            .map_err(|e| e.to_string())?;
    }
    #[cfg(target_os = "windows")]
    {
        std::process::Command::new("explorer")
            .arg(path)
            .spawn()
            .map_err(|e| e.to_string())?;
    }
    #[cfg(target_os = "linux")]
    {
        std::process::Command::new("xdg-open")
            .arg(path)
            .spawn()
            .map_err(|e| e.to_string())?;
    }
    Ok(())
}

pub fn run() {
    let app = tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_process::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .setup(|app| {
            let data_dir = app.path().app_data_dir().map_err(|e| e.to_string())?;
            std::fs::create_dir_all(&data_dir)?;
            let file = config::config_path(data_dir.clone());
            let cfg = config::load(&file).map_err(std::io::Error::other)?;
            let show_item = MenuItemBuilder::with_id("show", "显示 OnlyCodex").build(app)?;
            let quit_item = MenuItemBuilder::with_id("quit", "退出 OnlyCodex").build(app)?;
            let tray_menu = MenuBuilder::new(app)
                .items(&[&show_item, &quit_item])
                .build()?;
            let tray_icon = tauri::image::Image::from_bytes(include_bytes!("../tray-icon.png"))
                .map_err(|e| std::io::Error::other(format!("托盘图标加载失败: {e}")))?;
            TrayIconBuilder::new()
                .icon(tray_icon)
                .menu(&tray_menu)
                .show_menu_on_left_click(false)
                .tooltip("OnlyCodex")
                .on_menu_event(|app, event| match event.id().as_ref() {
                    "show" => {
                        if let Some(window) = app.get_webview_window("main") {
                            let _ = window.show();
                            let _ = window.unminimize();
                            let _ = window.set_focus();
                        }
                    }
                    "quit" => app.exit(0),
                    _ => {}
                })
                .on_tray_icon_event(|tray, event| {
                    if let TrayIconEvent::Click {
                        button: MouseButton::Left,
                        button_state: MouseButtonState::Up,
                        ..
                    } = event
                    {
                        if let Some(window) = tray.app_handle().get_webview_window("main") {
                            let _ = window.show();
                            let _ = window.unminimize();
                            let _ = window.set_focus();
                        }
                    }
                })
                .build(app)?;
            let auto = cfg.settings.start_on_launch;
            let runtime = Arc::new(AppRuntime {
                config: RwLock::new(cfg),
                config_file: file,
                data_dir,
                server: Mutex::new(None),
                logs: RwLock::new(VecDeque::new()),
                log_sequence: AtomicU64::new(1),
                requests: AtomicU64::new(0),
                errors: AtomicU64::new(0),
                client: reqwest::Client::builder()
                    .user_agent(concat!("OnlyCodex/", env!("CARGO_PKG_VERSION")))
                    .build()?,
            });
            runtime.info("OnlyCodex 已就绪".into());
            app.manage(runtime.clone());
            if auto {
                let rt = runtime.clone();
                tauri::async_runtime::spawn(async move {
                    match start_proxy_inner(&rt).await {
                        Ok(_) => rt.info("代理已自动启动".into()),
                        Err(e) => rt.error(e),
                    }
                });
            }
            Ok(())
        })
        .on_window_event(|window, event| {
            if let WindowEvent::CloseRequested { api, .. } = event {
                api.prevent_close();
                let _ = window.hide();
            }
        })
        .invoke_handler(tauri::generate_handler![
            get_config,
            save_config,
            get_status,
            start_proxy,
            stop_proxy,
            restore_codex_config,
            backup_codex_config,
            refresh_model_catalog,
            restart_codex,
            test_provider,
            get_logs,
            clear_logs,
            open_codex_config,
            reveal_data_dir
        ])
        .build(tauri::generate_context!())
        .expect("failed to build OnlyCodex");
    app.run(|app_handle, event| {
        if matches!(event, tauri::RunEvent::Exit) {
            let runtime = app_handle.state::<Arc<AppRuntime>>().inner().clone();
            tauri::async_runtime::block_on(async {
                if let Some(mut handle) = runtime.server.lock().await.take() {
                    handle.stop();
                }
            });
            if let Ok(path) = codex::default_config_path() {
                if let Err(error) = codex::restore(&path, &runtime.backup_path()) {
                    runtime.error(format!("退出时恢复 Codex 配置失败: {error}"));
                }
            }
        }
    });
}

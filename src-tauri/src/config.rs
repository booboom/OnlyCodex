use serde::{Deserialize, Serialize};
use std::{
    fs,
    path::{Path, PathBuf},
};
use uuid::Uuid;

pub const DEFAULT_CONTEXT_WINDOW: u64 = 1_050_000;
const LEGACY_DEFAULT_CONTEXT_WINDOW: u64 = 128_000;

fn default_context_window() -> u64 {
    DEFAULT_CONTEXT_WINDOW
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum Protocol {
    ChatCompletions,
    Responses,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelConfig {
    pub id: String,
    pub name: String,
    pub enabled: bool,
    #[serde(default = "default_context_window")]
    pub context_window: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Provider {
    pub id: String,
    pub name: String,
    pub base_url: String,
    #[serde(default)]
    pub api_key: String,
    #[serde(default = "default_protocol")]
    pub protocol: Protocol,
    pub enabled: bool,
    #[serde(default)]
    pub models: Vec<ModelConfig>,
}

fn default_protocol() -> Protocol {
    Protocol::Responses
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Mapping {
    pub id: String,
    pub codex_model: String,
    pub provider_id: String,
    pub upstream_model: String,
    pub enabled: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Settings {
    #[serde(default = "default_bind")]
    pub bind_address: String,
    #[serde(default = "default_port")]
    pub port: u16,
    #[serde(default = "default_timeout")]
    pub request_timeout_seconds: u64,
    #[serde(default = "default_max_body")]
    pub max_body_mb: usize,
    #[serde(default = "default_true")]
    pub restart_codex_on_change: bool,
    #[serde(default)]
    pub start_on_launch: bool,
}

fn default_bind() -> String {
    "127.0.0.1".into()
}
fn default_port() -> u16 {
    8787
}
fn default_timeout() -> u64 {
    120
}
fn default_max_body() -> usize {
    20
}
fn default_true() -> bool {
    true
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AppConfig {
    #[serde(default)]
    pub providers: Vec<Provider>,
    #[serde(default)]
    pub mappings: Vec<Mapping>,
    #[serde(default)]
    pub settings: Settings,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            bind_address: default_bind(),
            port: default_port(),
            request_timeout_seconds: default_timeout(),
            max_body_mb: default_max_body(),
            restart_codex_on_change: true,
            start_on_launch: false,
        }
    }
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            providers: vec![],
            mappings: vec![],
            settings: Settings {
                bind_address: "127.0.0.1".into(),
                port: 8787,
                request_timeout_seconds: 120,
                max_body_mb: 20,
                restart_codex_on_change: true,
                start_on_launch: false,
            },
        }
    }
}

pub fn load(path: &Path) -> Result<AppConfig, String> {
    if !path.exists() {
        return Ok(AppConfig::default());
    }
    let raw = fs::read_to_string(path).map_err(|e| format!("读取应用配置失败: {e}"))?;
    let mut config: AppConfig =
        serde_json::from_str(&raw).map_err(|e| format!("应用配置格式错误: {e}"))?;
    if normalize_context_windows(&mut config) {
        save(path, &config)?;
    }
    Ok(config)
}

/// Upgrade values written by older releases while preserving user-selected values.
pub fn normalize_context_windows(config: &mut AppConfig) -> bool {
    let mut changed = false;
    for provider in &mut config.providers {
        for model in &mut provider.models {
            if model.context_window == LEGACY_DEFAULT_CONTEXT_WINDOW {
                model.context_window = DEFAULT_CONTEXT_WINDOW;
                changed = true;
            }
        }
    }
    changed
}

pub fn save(path: &Path, config: &AppConfig) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| format!("创建数据目录失败: {e}"))?;
    }
    validate(config)?;
    let raw = serde_json::to_string_pretty(config).map_err(|e| e.to_string())?;
    let temp = path.with_extension(format!("{}.tmp", Uuid::new_v4()));
    fs::write(&temp, raw).map_err(|e| format!("写入临时配置失败: {e}"))?;
    fs::rename(&temp, path).map_err(|e| format!("原子保存配置失败: {e}"))
}

fn validate(config: &AppConfig) -> Result<(), String> {
    if config.settings.bind_address != "127.0.0.1" && config.settings.bind_address != "::1" {
        return Err("为保证安全，代理只能绑定 127.0.0.1 或 ::1".into());
    }
    if config.settings.port == 0 {
        return Err("端口必须在 1–65535 之间".into());
    }
    for provider in &config.providers {
        if provider.name.trim().is_empty() || provider.base_url.trim().is_empty() {
            return Err("供应商名称和 Base URL 不能为空".into());
        }
        if !provider.base_url.starts_with("http://") && !provider.base_url.starts_with("https://") {
            return Err(format!(
                "{} 的 Base URL 必须以 http:// 或 https:// 开头",
                provider.name
            ));
        }
    }
    for mapping in &config.mappings {
        if mapping.codex_model.trim().is_empty() || mapping.upstream_model.trim().is_empty() {
            return Err("映射模型名称不能为空".into());
        }
        if !config.providers.iter().any(|p| p.id == mapping.provider_id) {
            return Err(format!("映射 {} 引用了不存在的供应商", mapping.codex_model));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_context_window_uses_new_default() {
        let config: AppConfig = serde_json::from_str(
            r#"{"providers":[{"id":"p","name":"P","baseUrl":"https://example.com","enabled":true,"models":[{"id":"m","name":"M","enabled":true}]}]}"#,
        )
        .unwrap();
        assert_eq!(
            config.providers[0].models[0].context_window,
            DEFAULT_CONTEXT_WINDOW
        );
    }

    #[test]
    fn legacy_context_window_is_migrated_but_custom_value_is_preserved() {
        let mut config = AppConfig {
            providers: vec![Provider {
                id: "p".into(),
                name: "P".into(),
                base_url: "https://example.com".into(),
                api_key: String::new(),
                protocol: Protocol::Responses,
                enabled: true,
                models: vec![
                    ModelConfig {
                        id: "legacy".into(),
                        name: "Legacy".into(),
                        enabled: true,
                        context_window: LEGACY_DEFAULT_CONTEXT_WINDOW,
                    },
                    ModelConfig {
                        id: "custom".into(),
                        name: "Custom".into(),
                        enabled: true,
                        context_window: 64_000,
                    },
                ],
            }],
            ..AppConfig::default()
        };
        assert!(normalize_context_windows(&mut config));
        assert_eq!(
            config.providers[0].models[0].context_window,
            DEFAULT_CONTEXT_WINDOW
        );
        assert_eq!(config.providers[0].models[1].context_window, 64_000);
    }
}

pub fn config_path(app_data: PathBuf) -> PathBuf {
    app_data.join("config.json")
}

//! Synchronisation settings and Tauri commands.
//!
//! WebDAV is the *only* data source of this application: the library starts
//! empty and every note, folder, tag and attachment arrives through this module.

use crate::{
    error::{Error, Result},
    state::AppState,
    sync::engine::SyncResult,
};
use crate::sync::engine as sync_engine;
use serde::{Deserialize, Serialize};
use std::{fs, path::PathBuf};
use tauri::{AppHandle, State};

pub mod engine;
pub mod protocol;
pub mod webdav;

fn err(msg: impl Into<String>) -> Error {
    Error::msg(msg.into())
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct SyncConfig {
    pub url: String,
    pub username: String,
    pub password: String,
    pub allow_invalid_certs: bool,
    pub auto_sync: bool,
    pub interval_minutes: u32,
    pub last_sync_at: i64,
    pub last_message: String,
}

impl Default for SyncConfig {
    fn default() -> Self {
        Self {
            url: String::new(),
            username: String::new(),
            password: String::new(),
            allow_invalid_certs: false,
            auto_sync: false,
            interval_minutes: 10,
            last_sync_at: 0,
            last_message: String::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct SyncStatus {
    pub configured: bool,
    pub url: String,
    pub username: String,
    pub auto_sync: bool,
    pub interval_minutes: u32,
    pub last_sync_at: i64,
    pub last_message: String,
}

fn config_path() -> PathBuf {
    crate::paths::paths().sync_config
}

pub fn load_config() -> Result<SyncConfig> {
    let path = config_path();
    if !path.exists() {
        let cfg = SyncConfig::default();
        save_config(&cfg)?;
        return Ok(cfg);
    }
    let raw = fs::read_to_string(&path)?;
    serde_json::from_str(&raw).map_err(|e| err(format!("sync.json 损坏: {e}")))
}

pub fn save_config(cfg: &SyncConfig) -> Result<()> {
    let dir = crate::paths::data_dir();
    fs::create_dir_all(&dir)?;
    let text = serde_json::to_string_pretty(cfg)
        .map_err(|e| err(format!("序列化同步配置失败: {e}")))?;
    fs::write(config_path(), text)?;
    Ok(())
}

fn status_from(cfg: &SyncConfig) -> SyncStatus {
    SyncStatus {
        configured: !cfg.url.is_empty() && !cfg.password.is_empty(),
        url: cfg.url.clone(),
        username: cfg.username.clone(),
        auto_sync: cfg.auto_sync,
        interval_minutes: cfg.interval_minutes,
        last_sync_at: cfg.last_sync_at,
        last_message: cfg.last_message.clone(),
    }
}

#[tauri::command]
pub fn sync_get_config() -> Result<SyncConfig> {
    load_config()
}

#[tauri::command]
pub fn sync_status() -> Result<SyncStatus> {
    Ok(status_from(&load_config()?))
}

#[tauri::command]
pub fn sync_set_config(config: SyncConfig) -> Result<SyncStatus> {
    let mut cfg = config;
    cfg.url = cfg.url.trim().to_string();
    if cfg.interval_minutes < 1 {
        cfg.interval_minutes = 1;
    }
    save_config(&cfg)?;
    crate::startup_log(&format!(
        "sync config saved: url={} user={} auto={} interval={}min",
        cfg.url, cfg.username, cfg.auto_sync, cfg.interval_minutes
    ));
    Ok(status_from(&cfg))
}

#[tauri::command]
pub async fn sync_test(config: SyncConfig) -> std::result::Result<String, String> {
    tauri::async_runtime::spawn_blocking(move || sync_engine::test_connection(&config))
        .await
        .map_err(|e| e.to_string())?
        .map_err(|e| e.to_string())
}

async fn do_sync(
    app: AppHandle,
    state: State<'_, AppState>,
    dry: bool,
) -> std::result::Result<SyncResult, String> {
    let cfg = load_config().map_err(|e| e.to_string())?;
    if cfg.url.is_empty() {
        return Err("尚未配置 WebDAV 地址".to_string());
    }
    if cfg.password.is_empty() {
        return Err("尚未填写 WebDAV 密码，请在「同步」设置里补全".to_string());
    }
    let (work_db, resources) = {
        let p = state.cfg.read();
        (p.work_db.clone(), p.resources.clone())
    };
    crate::startup_log(&format!(
        "sync start: dry={} url={} user={}",
        dry, cfg.url, cfg.username
    ));
    let result = tauri::async_runtime::spawn_blocking(move || {
        sync_engine::run(&app, &work_db, &resources, &cfg, dry)
    })
    .await
    .map_err(|e| e.to_string())?
    .map_err(|e| e.to_string())?;

    if !dry {
        let mut saved = load_config().map_err(|e| e.to_string())?;
        saved.last_sync_at = result.finished_at;
        saved.last_message = result.message.clone();
        save_config(&saved).map_err(|e| e.to_string())?;
        // The engine wrote through its own connection; rebuild the search
        // index and attachment map so freshly downloaded notes are usable.
        if let Err(e) = state.reload() {
            crate::startup_log(&format!("state reload after sync failed: {e}"));
        }
    }
    crate::startup_log(&format!("sync end: {}", result.message));
    Ok(result)
}

#[tauri::command]
pub async fn sync_preview(
    app: AppHandle,
    state: State<'_, AppState>,
) -> std::result::Result<SyncResult, String> {
    do_sync(app, state, true).await
}

#[tauri::command]
pub async fn sync_run(
    app: AppHandle,
    state: State<'_, AppState>,
) -> std::result::Result<SyncResult, String> {
    do_sync(app, state, false).await
}

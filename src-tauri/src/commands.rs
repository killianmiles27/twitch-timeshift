//! Tauri command surface — the names here are the frozen frontend contract.

use std::path::PathBuf;
use std::time::Duration;

use chrono::{SecondsFormat, Utc};
use tauri::{AppHandle, Manager, Url, WebviewUrl, WebviewWindowBuilder};

use crate::config_store;
use crate::credentials;
use crate::models::{AppConfig, CredentialsStatus, StatusReport};
use crate::twitch;

/// Player window size per SPEC.
const PLAYER_WIDTH: f64 = 1_280.0;
const PLAYER_HEIGHT: f64 = 720.0;
/// How long to wait for a closed player window to release its label.
const PLAYER_CLOSE_TIMEOUT: Duration = Duration::from_millis(1_000);
const PLAYER_CLOSE_POLL: Duration = Duration::from_millis(25);
const PLAYER_BUILD_ATTEMPTS: usize = 5;
const PLAYER_BUILD_RETRY_DELAY: Duration = Duration::from_millis(100);

fn config_dir(app: &AppHandle) -> Result<PathBuf, String> {
    app.path()
        .app_config_dir()
        .map_err(|error| format!("Could not resolve the app config directory: {error}"))
}

/// `get_config() -> AppConfig`
#[tauri::command]
pub fn get_config(app: AppHandle) -> AppConfig {
    match config_dir(&app) {
        Ok(dir) => config_store::load_config(&dir),
        // A missing config dir must not break the first launch; the UI can still
        // render defaults and saving will report the real error.
        Err(_) => config_store::default_config(),
    }
}

/// `save_config(config: AppConfig) -> AppConfig`
#[tauri::command]
pub fn save_config(app: AppHandle, config: AppConfig) -> Result<AppConfig, String> {
    let dir = config_dir(&app)?;
    config_store::save_config(&dir, &config)
}

/// `get_statuses() -> StatusReport`
#[tauri::command]
pub async fn get_statuses(app: AppHandle) -> Result<StatusReport, String> {
    let dir = config_dir(&app)?;
    let config = config_store::load_config(&dir);

    let credentials = credentials::load(Some(dir.as_path())).ok_or_else(|| {
        "No Twitch API credentials found. Set TWITCH_CLIENT_ID and TWITCH_CLIENT_SECRET, \
         or add .twitch-secrets.json to the app config directory."
            .to_string()
    })?;

    let entries = twitch::fetch_statuses(&config, &credentials).await?;

    Ok(StatusReport {
        generated_at: Utc::now().to_rfc3339_opts(SecondsFormat::Secs, true),
        timezone: config.timezone,
        offset_hours: config.offset_hours,
        entries,
    })
}

/// `open_player(vodId: string, joinSeconds: number) -> void`
///
/// Creates (or rebuilds, so the position is refreshed) the dedicated player
/// window labelled `player-{vodId}`.
#[tauri::command]
pub async fn open_player(
    app: AppHandle,
    vod_id: String,
    join_seconds: u64,
) -> Result<(), String> {
    let vod_id = vod_id.trim().to_string();
    if vod_id.is_empty() {
        return Err("open_player needs a VOD id.".to_string());
    }

    let label = format!("player-{vod_id}");
    let url = Url::parse(&twitch::player_url(&vod_id, join_seconds))
        .map_err(|error| format!("Could not build the player URL: {error}"))?;

    if let Some(existing) = app.get_webview_window(&label) {
        let _ = existing.destroy();
        // The label stays registered until the event loop destroys the window;
        // wait (bounded) so the rebuild below does not collide with it.
        if !wait_for_window_release(&app, &label, PLAYER_CLOSE_TIMEOUT).await {
            return Err("The previous player window did not close in time.".to_string());
        }
    }

    let mut last_error: Option<String> = None;
    for attempt in 0..PLAYER_BUILD_ATTEMPTS {
        if attempt > 0 {
            tokio::time::sleep(PLAYER_BUILD_RETRY_DELAY).await;
        }
        match WebviewWindowBuilder::new(&app, label.clone(), WebviewUrl::External(url.clone()))
            .title(vod_id.clone())
            .inner_size(PLAYER_WIDTH, PLAYER_HEIGHT)
            .resizable(true)
            .build()
        {
            Ok(window) => {
                let _ = window.show();
                let _ = window.set_focus();
                return Ok(());
            }
            Err(error) => last_error = Some(error.to_string()),
        }
    }

    Err(format!(
        "Could not open the player window: {}",
        last_error.unwrap_or_else(|| "unknown error".to_string())
    ))
}

/// `get_credentials_status() -> CredentialsStatus`
#[tauri::command]
pub fn get_credentials_status(app: AppHandle) -> CredentialsStatus {
    let dir = config_dir(&app).ok();
    credentials::status(dir.as_deref())
}

async fn wait_for_window_release(app: &AppHandle, label: &str, timeout: Duration) -> bool {
    let step = PLAYER_CLOSE_POLL;
    let mut waited = Duration::ZERO;
    while waited < timeout {
        if app.get_webview_window(label).is_none() {
            return true;
        }
        tokio::time::sleep(step).await;
        waited += step;
    }
    app.get_webview_window(label).is_none()
}

//! Tauri command surface — the names here are the frontend contract.

use std::path::PathBuf;
use std::sync::Mutex;
use std::time::Duration;

use chrono::{DateTime, SecondsFormat, Utc};
use tauri::{AppHandle, Emitter, Manager, State, WebviewUrl, WebviewWindowBuilder};

use crate::config_store;
use crate::credentials;
use crate::models::{
    AppConfig, ConfigLoad, CredentialsStatus, OpenPlayerResult, StatusReport, StreamerStatus,
};
use crate::twitch;

/// Player window size.
const PLAYER_WIDTH: f64 = 1_280.0;
const PLAYER_HEIGHT: f64 = 720.0;
/// How long to wait for a closed player window to release its label.
const PLAYER_CLOSE_TIMEOUT: Duration = Duration::from_millis(1_000);
const PLAYER_CLOSE_POLL: Duration = Duration::from_millis(25);
const PLAYER_BUILD_ATTEMPTS: usize = 5;
const PLAYER_BUILD_RETRY_DELAY: Duration = Duration::from_millis(100);

/// Last computed statuses, so `open_player` can recompute the authoritative
/// join position instead of trusting a stale frontend value (BUG_REVIEW #7).
#[derive(Default)]
pub struct StatusCache {
    entries: Mutex<Vec<StreamerStatus>>,
}

fn config_dir(app: &AppHandle) -> Result<PathBuf, String> {
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = config_store::portable_config_dir(&exe) {
            return Ok(dir);
        }
    }
    app.path()
        .app_config_dir()
        .map_err(|error| format!("Could not resolve the app config directory: {error}"))
}

/// `get_config() -> ConfigLoad`
#[tauri::command]
pub fn get_config(app: AppHandle) -> ConfigLoad {
    match config_dir(&app) {
        Ok(dir) => config_store::load_config_detailed(&dir),
        Err(error) => ConfigLoad {
            config: config_store::default_config(),
            first_launch: false,
            recovered: true,
            warning: Some(error),
        },
    }
}

/// `save_config(config: AppConfig) -> AppConfig`
#[tauri::command]
pub fn save_config(
    app: AppHandle,
    cache: State<'_, StatusCache>,
    config: AppConfig,
) -> Result<AppConfig, String> {
    let dir = config_dir(&app)?;
    let saved = config_store::save_config(&dir, &config)?;
    if let Ok(entries) = cache.entries.lock() {
        retarget_players(&app, &entries, &saved);
    }
    Ok(saved)
}

fn retarget_players(app: &AppHandle, entries: &[StreamerStatus], config: &AppConfig) {
    for entry in entries {
        let Some(vod) = &entry.vod else { continue };
        let label = format!("player-{}", vod.id);
        if app.get_webview_window(&label).is_none() {
            continue;
        }
        if let Ok(created) = DateTime::parse_from_rfc3339(&vod.created_at) {
            let window = crate::timeshift::evaluate(
                Utc::now(),
                created.with_timezone(&Utc),
                vod.duration_seconds,
                config.offset_hours,
            );
            let _ = app.emit_to(
                &label,
                "player-retarget",
                serde_json::json!({
                    "vodId": vod.id, "startMs": window.sim_start.timestamp_millis(),
                    "durationSeconds": vod.duration_seconds,
                }),
            );
        }
    }
}

/// `get_statuses() -> StatusReport`
#[tauri::command]
pub async fn get_statuses(
    app: AppHandle,
    cache: State<'_, StatusCache>,
) -> Result<StatusReport, String> {
    let dir = config_dir(&app)?;
    let config = config_store::load_config(&dir);

    if config.streamers.is_empty() {
        if let Ok(mut slot) = cache.entries.lock() {
            slot.clear();
        }
        return Ok(StatusReport {
            generated_at: Utc::now().to_rfc3339_opts(SecondsFormat::Secs, true),
            timezone: config.timezone,
            offset_hours: config.offset_hours,
            entries: Vec::new(),
        });
    }

    let credentials = credentials::load(Some(dir.as_path())).ok_or_else(|| {
        "No Twitch API credentials found. Set TWITCH_CLIENT_ID and TWITCH_CLIENT_SECRET, \
         or add twitch-secrets.json to the app config directory."
            .to_string()
    })?;

    let (entries, classified_at) = twitch::fetch_statuses(&config, &credentials).await?;

    if let Ok(mut slot) = cache.entries.lock() {
        *slot = entries.clone();
    }
    retarget_players(&app, &entries, &config);

    Ok(StatusReport {
        generated_at: classified_at.to_rfc3339_opts(SecondsFormat::Secs, true),
        timezone: config.timezone,
        offset_hours: config.offset_hours,
        entries,
    })
}

/// `open_player(vodId: string) -> OpenPlayerResult`
///
/// The join position is computed here, from the last report and a fresh
/// clock — never accepted from the frontend. Opening outside the simulated
/// window is an error, not a best effort (BUG_REVIEW #7). The window loads
/// the bundled `player.html` wrapper (supported `Twitch.Player` embed);
/// window creation only means "opened" — playback states arrive separately
/// through `report_player_state` (BUG_REVIEW #9).
#[tauri::command]
pub async fn open_player(
    app: AppHandle,
    cache: State<'_, StatusCache>,
    vod_id: String,
) -> Result<OpenPlayerResult, String> {
    let vod_id = vod_id.trim().to_string();
    if vod_id.is_empty() {
        return Err("open_player needs a VOD id.".to_string());
    }

    let entry = {
        let entries = cache
            .entries
            .lock()
            .map_err(|_| "The status cache is unusable.".to_string())?;
        entries
            .iter()
            .find(|e| e.vod.as_ref().is_some_and(|vod| vod.id == vod_id))
            .cloned()
            .ok_or_else(|| {
                "That VOD is not in the current schedule. Refresh and try again.".to_string()
            })?
    };

    let Some(vod) = entry.vod.clone() else {
        return Err("That VOD has no simulated window. Refresh and try again.".to_string());
    };
    let config = config_store::load_config(&config_dir(&app)?);
    if !config.streamers.contains(&entry.login) {
        return Err("This streamer was removed. Refresh the schedule.".to_string());
    }
    let created = DateTime::parse_from_rfc3339(&vod.created_at)
        .map_err(|_| "The scheduled start could not be read. Refresh and try again.".to_string())?
        .with_timezone(&Utc);
    let evaluation = crate::timeshift::evaluate(
        Utc::now(),
        created,
        vod.duration_seconds,
        config.offset_hours,
    );
    let sim_start = evaluation.sim_start;
    if evaluation.state == crate::timeshift::TimeshiftState::Upcoming {
        return Err(format!(
            "This replay has not started yet. It starts at {sim_start}."
        ));
    }
    if evaluation.state != crate::timeshift::TimeshiftState::Live {
        return Err(
            "This replay window has ended. Refresh to see the current schedule.".to_string(),
        );
    }
    let join_seconds = evaluation.join_seconds.unwrap_or(0) as u64;

    let label = format!("player-{vod_id}");
    let start_ms = sim_start.timestamp_millis();

    // Already open: retarget the running player to the current position
    // instead of destroying it (settings may have changed since it opened).
    if let Some(existing) = app.get_webview_window(&label) {
        let _ = app.emit_to(
            &label,
            "player-retarget",
            serde_json::json!({
                "vodId": vod_id,
                "startMs": start_ms,
                "durationSeconds": vod.duration_seconds,
            }),
        );
        let _ = existing.show();
        let _ = existing.set_focus();
        return Ok(OpenPlayerResult {
            vod_id,
            join_seconds,
        });
    }

    let fallback_url = twitch::player_url(&vod_id, join_seconds);
    let app_url = format!(
        "player.html?vod={vod_id}&start={start_ms}&dur={}&fb={}",
        vod.duration_seconds,
        percent_encode(&fallback_url)
    );

    let mut last_error: Option<String> = None;
    for attempt in 0..PLAYER_BUILD_ATTEMPTS {
        if attempt > 0 {
            tokio::time::sleep(PLAYER_BUILD_RETRY_DELAY).await;
        }
        match WebviewWindowBuilder::new(
            &app,
            label.clone(),
            WebviewUrl::App(app_url.clone().into()),
        )
        .title(format!("Twitch Timeshift Player - {vod_id}"))
        .use_https_scheme(true)
        .inner_size(PLAYER_WIDTH, PLAYER_HEIGHT)
        .resizable(true)
        .build()
        {
            Ok(window) => {
                let _ = window.show();
                let _ = window.set_focus();
                return Ok(OpenPlayerResult {
                    vod_id,
                    join_seconds,
                });
            }
            Err(error) => last_error = Some(error.to_string()),
        }
    }

    Err(format!(
        "Could not open the player window: {}",
        last_error.unwrap_or_else(|| "unknown error".to_string())
    ))
}

/// `report_player_state(vodId, state, detail?)` — called by the player
/// wrapper as playback progresses (opened/ready/playing/blocked/failed…).
/// Forwarded to the main window as a `player-state` event. Never logs or
/// returns credential material.
#[tauri::command]
pub fn report_player_state(
    app: AppHandle,
    vod_id: String,
    state: String,
    detail: Option<String>,
) -> Result<(), String> {
    app.emit(
        "player-state",
        serde_json::json!({
            "vodId": vod_id,
            "state": state,
            "detail": detail,
        }),
    )
    .map_err(|error| format!("Could not report player state: {error}"))
}

/// `close_player(vodId)` — used when a replay window ends while watching.
#[tauri::command]
pub fn close_player(app: AppHandle, vod_id: String) -> Result<(), String> {
    let label = format!("player-{}", vod_id.trim());
    if let Some(window) = app.get_webview_window(&label) {
        let _ = window.destroy();
        // Wait briefly so an immediate re-open does not collide on the label.
        let deadline = std::time::Instant::now() + PLAYER_CLOSE_TIMEOUT;
        while std::time::Instant::now() < deadline {
            if app.get_webview_window(&label).is_none() {
                break;
            }
            std::thread::sleep(PLAYER_CLOSE_POLL);
        }
    }
    Ok(())
}

/// Minimal percent-encoding for embedding a URL inside a query value.
fn percent_encode(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(byte as char)
            }
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

/// `get_credentials_status() -> CredentialsStatus`
#[tauri::command]
pub fn get_credentials_status(app: AppHandle) -> CredentialsStatus {
    let dir = config_dir(&app).ok();
    credentials::status(dir.as_deref())
}

fn twitch_web_url(vod_id: &str) -> Result<String, String> {
    if vod_id.is_empty() || vod_id.len() > 32 || !vod_id.bytes().all(|b| b.is_ascii_digit()) {
        return Err("A numeric Twitch VOD id is required.".to_string());
    }
    Ok(format!("https://www.twitch.tv/videos/{vod_id}"))
}

/// Open only a validated Twitch video URL in the user's normal browser.
/// WebView2 ignores target=_blank links without a new-window handler.
#[tauri::command]
pub fn open_on_twitch(vod_id: String) -> Result<(), String> {
    let url = twitch_web_url(&vod_id)?;
    #[cfg(target_os = "windows")]
    let mut command = {
        use std::os::windows::process::CommandExt;
        let mut cmd = std::process::Command::new("explorer.exe");
        cmd.creation_flags(0x08000000);
        cmd
    };
    #[cfg(target_os = "macos")]
    let mut command = std::process::Command::new("open");
    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    let mut command = std::process::Command::new("xdg-open");
    command
        .arg(url)
        .spawn()
        .map(|_| ())
        .map_err(|error| format!("Could not open your browser: {error}"))
}

#[cfg(test)]
mod tests {
    #[test]
    fn browser_recovery_only_accepts_numeric_video_ids() {
        assert_eq!(
            super::twitch_web_url("12345").unwrap(),
            "https://www.twitch.tv/videos/12345"
        );
        for bad in [
            "",
            "../settings",
            "123&other=1",
            "https://example.com",
            "123 456",
        ] {
            assert!(super::twitch_web_url(bad).is_err());
        }
    }
}

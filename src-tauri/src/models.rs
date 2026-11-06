//! Shared data contract between the Rust backend and the web frontend.
//! Mirrors SPEC.md ("Frozen contract") exactly. JSON is camelCase.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct AppConfig {
    pub streamers: Vec<String>,
    pub timezone: String,
    pub offset_hours: f64,
    pub auto_refresh_seconds: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct VodInfo {
    pub id: String,
    pub title: String,
    pub url: String,
    pub thumbnail_url: Option<String>,
    /// RFC 3339 UTC instant the original stream started.
    pub created_at: String,
    pub duration_seconds: u64,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum StreamerState {
    Live,
    Upcoming,
    Offline,
    Novod,
    Notfound,
    Error,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct StreamerStatus {
    pub login: String,
    pub display_name: String,
    pub avatar_url: Option<String>,
    pub state: StreamerState,
    pub vod: Option<VodInfo>,
    /// RFC 3339 UTC simulated window start (`vod.created_at + offset`).
    pub sim_start: Option<String>,
    /// RFC 3339 UTC simulated window end.
    pub sim_end: Option<String>,
    /// Seconds into the VOD if opened now (live only).
    pub join_seconds: Option<i64>,
    /// Seconds until the simulated start (upcoming only).
    pub starts_in_seconds: Option<i64>,
    /// 0.0..=1.0 through the VOD (live only).
    pub progress: Option<f64>,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct StatusReport {
    pub generated_at: String,
    pub timezone: String,
    pub offset_hours: f64,
    pub entries: Vec<StreamerStatus>,
}

/// Never carries credential values — only whether/where they were found.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct CredentialsStatus {
    pub configured: bool,
    /// One of: "environment", "file", "embedded", "missing".
    pub source: String,
    /// Whether Twitch has accepted these credentials in this session.
    /// `configured` alone does not prove that (BUG_REVIEW #16); the backend
    /// reports `false` and the frontend flips it after a successful report.
    pub authenticated: bool,
}

/// Result of loading the persisted config (BUG_REVIEW #12).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ConfigLoad {
    pub config: AppConfig,
    /// No config file existed: defaults are a starting point, not a reset.
    pub first_launch: bool,
    /// An existing file could not be used as-is (corrupt/unreadable); a
    /// backup was kept next to it and `warning` explains what happened.
    pub recovered: bool,
    pub warning: Option<String>,
}

/// Returned by `open_player`: the backend-computed authoritative position.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct OpenPlayerResult {
    pub vod_id: String,
    pub join_seconds: u64,
}

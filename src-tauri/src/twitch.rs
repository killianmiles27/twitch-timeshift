//! Twitch Helix client: app-access-token cache, users, latest VODs and the
//! aggregated status report.
//!
//! Credentials are only ever read through [`Credentials`]; they are never put
//! into URLs, error messages or logs.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex as StdMutex, OnceLock};
use std::time::{Duration, Instant};

use chrono::{DateTime, SecondsFormat, Utc};
use futures::stream::{self, StreamExt};
use reqwest::{Client, StatusCode};
use serde::de::DeserializeOwned;
use serde::Deserialize;
use tokio::sync::Mutex;

use crate::credentials::Credentials;
use crate::models::{AppConfig, StreamerState, StreamerStatus, VodInfo};
use crate::timeshift::{self, Evaluation, TimeshiftState};

// Re-exported so the duration parser is reachable from this module too
// (SPEC.md assigns it here, the pure implementation lives in `timeshift`).
pub use crate::timeshift::parse_duration;

const TOKEN_URL: &str = "https://id.twitch.tv/oauth2/token";
const HELIX_BASE: &str = "https://api.twitch.tv/helix";
const PLAYER_BASE: &str = "https://player.twitch.tv/";
const PLAYER_PARENT: &str = "localhost";
const VOD_WEB_BASE: &str = "https://www.twitch.tv/videos";
/// Helix accepts at most 100 `login` parameters per request.
const MAX_USERS_PER_REQUEST: usize = 100;
/// Concurrent VOD lookups per status report.
const MAX_CONCURRENT_VOD_REQUESTS: usize = 8;
/// Broadcasts per history page (Helix maximum is 100).
const VOD_PAGE_SIZE: usize = 100;
/// Hard cap on history pages per streamer per refresh.
const MAX_VOD_PAGES: usize = 10;
/// Refresh the app token this long before it actually expires.
const TOKEN_REFRESH_MARGIN: Duration = Duration::from_secs(60);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(15);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
/// How much of an error body is echoed back (never contains credentials).
const MAX_ERROR_BODY_CHARS: usize = 200;

/// A Twitch user resolved from a login name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TwitchUser {
    pub id: String,
    pub login: String,
    pub display_name: String,
    pub profile_image_url: Option<String>,
}

struct CachedToken {
    value: String,
    /// Stop handing this token out after this instant (before true expiry).
    refresh_after: Instant,
    /// True server-side expiry; the token is never used past this instant.
    expires_at: Instant,
}

/// Helix client with a cached app access token.
pub struct TwitchClient {
    http: Client,
    credentials: Credentials,
    token: Mutex<Option<CachedToken>>,
}

#[derive(Deserialize)]
struct TokenResponse {
    access_token: Option<String>,
    expires_in: Option<u64>,
}

/// Parse a token endpoint body into `(token, lifetime)`.
///
/// Malformed responses — missing/empty token, missing or zero `expires_in` —
/// are explicit errors, never silently guessed (BUG_REVIEW #18).
fn parse_token_body(body: &str) -> Result<(String, Duration), String> {
    let parsed: TokenResponse = serde_json::from_str(body)
        .map_err(|error| format!("Twitch returned an unreadable app token response: {error}"))?;
    let value = parsed
        .access_token
        .filter(|token| !token.trim().is_empty())
        .ok_or_else(|| "Twitch returned an empty app token.".to_string())?;
    let ttl_seconds = parsed
        .expires_in
        .filter(|seconds| *seconds > 0)
        .ok_or_else(|| "Twitch returned a token without a valid expiry.".to_string())?;
    Ok((value, Duration::from_secs(ttl_seconds)))
}

/// `(usable lifetime, hard lifetime)` for a token TTL.
///
/// The token stops being handed out `min(60s, ttl/4)` before it expires, and
/// is never used past its true expiry — a 20-second token is usable for 15.
fn token_lifetimes(ttl: Duration) -> (Duration, Duration) {
    let margin = TOKEN_REFRESH_MARGIN.min(ttl / 4);
    (ttl.saturating_sub(margin), ttl)
}

#[derive(Deserialize)]
struct UsersResponse {
    data: Vec<RawUser>,
}

#[derive(Deserialize)]
struct RawUser {
    id: String,
    login: String,
    #[serde(default)]
    display_name: String,
    #[serde(default)]
    profile_image_url: Option<String>,
}

#[derive(Deserialize)]
struct VideosResponse {
    data: Vec<RawVideo>,
    #[serde(default)]
    pagination: Option<RawPagination>,
}

#[derive(Deserialize)]
struct RawPagination {
    #[serde(default)]
    cursor: Option<String>,
}

#[derive(Deserialize)]
struct RawVideo {
    id: String,
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    url: Option<String>,
    #[serde(default)]
    thumbnail_url: Option<String>,
    #[serde(default)]
    created_at: String,
    #[serde(default)]
    duration: String,
}

impl TwitchClient {
    /// Build a client for the given credentials. No network I/O happens here.
    pub fn new(credentials: &Credentials) -> Result<Self, String> {
        if credentials.client_id.trim().is_empty() || credentials.client_secret.trim().is_empty() {
            return Err("The configured Twitch credentials are incomplete.".to_string());
        }

        let http = Client::builder()
            .timeout(REQUEST_TIMEOUT)
            .connect_timeout(CONNECT_TIMEOUT)
            .user_agent(concat!("twitch-timeshift/", env!("CARGO_PKG_VERSION")))
            .build()
            .map_err(|error| format!("Could not create the HTTP client: {error}"))?;

        Ok(Self {
            http,
            credentials: credentials.clone(),
            token: Mutex::new(None),
        })
    }

    /// Return a valid app access token, fetching (and caching) one if needed.
    async fn access_token(&self) -> Result<String, String> {
        let mut cached_slot = self.token.lock().await;
        if let Some(cached) = cached_slot.as_ref() {
            let now = Instant::now();
            if now < cached.refresh_after && now < cached.expires_at {
                return Ok(cached.value.clone());
            }
        }

        let response = self
            .http
            .post(TOKEN_URL)
            .form(&[
                ("client_id", self.credentials.client_id.as_str()),
                ("client_secret", self.credentials.client_secret.as_str()),
                ("grant_type", "client_credentials"),
            ])
            .send()
            .await
            .map_err(|error| format!("Could not reach Twitch to request an app token: {error}"))?;

        let status = response.status();
        if !status.is_success() {
            // Never echo this response: it belongs to a request carrying secrets.
            return Err(format!(
                "Twitch rejected the app token request (HTTP {}). Check the configured client id and secret.",
                status.as_u16()
            ));
        }

        let body = response.text().await.map_err(|error| {
            format!("Twitch returned an unreadable app token response: {error}")
        })?;
        let (value, ttl) = parse_token_body(&body)?;

        let fetched_at = Instant::now();
        let (usable, hard) = token_lifetimes(ttl);
        *cached_slot = Some(CachedToken {
            value: value.clone(),
            refresh_after: fetched_at + usable,
            expires_at: fetched_at + hard,
        });

        Ok(value)
    }

    /// Forget the cached token only if it is still the token that was used.
    /// A late 401 must not clear a token another request already refreshed
    /// (BUG_REVIEW #18).
    async fn invalidate_token(&self, used_token: &str) {
        let mut cached_slot = self.token.lock().await;
        if cached_slot
            .as_ref()
            .is_some_and(|cached| cached.value == used_token)
        {
            *cached_slot = None;
        }
    }

    /// GET a Helix endpoint, retrying exactly once after a 401.
    async fn get_json<T: DeserializeOwned>(
        &self,
        path: &str,
        query: Vec<(String, String)>,
    ) -> Result<T, String> {
        let mut retried = false;
        loop {
            let token = self.access_token().await?;
            let response = self
                .http
                .get(format!("{HELIX_BASE}{path}"))
                .query(&query)
                .header("Client-Id", self.credentials.client_id.as_str())
                .header("Authorization", format!("Bearer {token}"))
                .send()
                .await
                .map_err(|error| format!("Twitch request to {path} failed: {error}"))?;

            let status = response.status();

            if status == StatusCode::UNAUTHORIZED && !retried {
                retried = true;
                self.invalidate_token(&token).await;
                continue;
            }

            if !status.is_success() {
                let detail = response.text().await.unwrap_or_default();
                return Err(format!(
                    "Twitch returned HTTP {} for {path}{}",
                    status.as_u16(),
                    body_preview(&detail)
                ));
            }

            return response
                .json::<T>()
                .await
                .map_err(|error| format!("Could not read Twitch's {path} response: {error}"));
        }
    }

    /// Resolve logins to users, batching at most 100 logins per request.
    /// Keys of the returned map are lowercased logins.
    pub async fn fetch_users(
        &self,
        logins: &[String],
    ) -> Result<HashMap<String, TwitchUser>, String> {
        let mut unique: Vec<String> = Vec::new();
        let mut seen: HashSet<String> = HashSet::new();
        for raw in logins {
            let login = raw.trim().trim_start_matches('@').to_ascii_lowercase();
            if !login.is_empty() && seen.insert(login.clone()) {
                unique.push(login);
            }
        }

        let mut users: HashMap<String, TwitchUser> = HashMap::new();
        for chunk in unique.chunks(MAX_USERS_PER_REQUEST) {
            let mut query: Vec<(String, String)> = Vec::with_capacity(chunk.len());
            for login in chunk {
                query.push(("login".to_string(), login.clone()));
            }

            let response: UsersResponse = self.get_json("/users", query).await?;
            for raw in response.data {
                let login = raw.login.to_ascii_lowercase();
                let display_name = if raw.display_name.trim().is_empty() {
                    login.clone()
                } else {
                    raw.display_name
                };
                users.insert(
                    login.clone(),
                    TwitchUser {
                        id: raw.id,
                        login,
                        display_name,
                        profile_image_url: raw
                            .profile_image_url
                            .filter(|url| !url.trim().is_empty()),
                    },
                );
            }
        }

        Ok(users)
    }

    /// Paginated archived-broadcast history for a user, newest first.
    ///
    /// Pages until the oldest fetched broadcast's shifted window has fully
    /// ended (older pages can only matter via implausibly long durations),
    /// the history runs out, or [`MAX_VOD_PAGES`] is reached — enough to cover
    /// any configured delay up to 168h for realistic streaming cadences
    /// (BUG_REVIEW #1). Durations are fetched fresh on every call, so a
    /// still-growing in-progress broadcast is re-measured each refresh.
    pub async fn fetch_vod_history(
        &self,
        user_id: &str,
        offset_hours: f64,
    ) -> Result<Vec<VodInfo>, String> {
        let fetch_started = Utc::now();
        let mut history: Vec<VodInfo> = Vec::new();
        let mut after: Option<String> = None;

        for _page in 0..MAX_VOD_PAGES {
            let mut query = vec![
                ("user_id".to_string(), user_id.to_string()),
                ("type".to_string(), "archive".to_string()),
                ("first".to_string(), VOD_PAGE_SIZE.to_string()),
                ("sort".to_string(), "time".to_string()),
            ];
            if let Some(cursor) = &after {
                query.push(("after".to_string(), cursor.clone()));
            }

            let response: VideosResponse = self.get_json("/videos", query).await?;
            let page_len = response.data.len();
            let mut oldest_end: Option<DateTime<Utc>> = None;
            for raw in response.data {
                match vod_from_raw(raw) {
                    Ok(vod) => {
                        if let Ok(created) = DateTime::parse_from_rfc3339(&vod.created_at) {
                            let end = timeshift::evaluate(
                                fetch_started,
                                created.with_timezone(&Utc),
                                vod.duration_seconds,
                                offset_hours,
                            )
                            .sim_end;
                            oldest_end = Some(end);
                        }
                        history.push(vod);
                    }
                    Err(error) => return Err(error),
                }
            }

            if page_len < VOD_PAGE_SIZE {
                break;
            }
            // Everything older shifted-ends even earlier; once the oldest
            // window on this page has ended, history is deep enough.
            if oldest_end.is_some_and(|end| end <= fetch_started) {
                break;
            }
            after = response
                .pagination
                .and_then(|p| p.cursor)
                .filter(|cursor| !cursor.trim().is_empty());
            if after.is_none() {
                break;
            }
        }

        Ok(history)
    }

    /// Build the status report entries for the configured streamers.
    ///
    /// Returns the entries plus the instant they were classified against —
    /// a fresh timestamp taken *after* all network requests complete, so a
    /// slow fetch cannot classify against a stale "now" (BUG_REVIEW #13).
    /// Order: live, then upcoming by simulated start, then offline, novod,
    /// notfound, error; alphabetical by login within the remaining groups.
    pub async fn statuses(
        &self,
        config: &AppConfig,
    ) -> Result<(Vec<StreamerStatus>, DateTime<Utc>), String> {
        let logins = normalize_login_list(&config.streamers);
        if logins.is_empty() {
            return Ok((Vec::new(), Utc::now()));
        }

        // A credential / token problem is fatal: every streamer would fail the
        // same way, so the caller gets one clear error instead of N.
        self.access_token().await?;

        // Any other users-request failure only degrades the report: every
        // streamer becomes an `error` entry instead of killing everything.
        let users = match self.fetch_users(&logins).await {
            Ok(users) => users,
            Err(error) => return Ok((all_errors(&logins, &error), Utc::now())),
        };

        let mut entries: Vec<StreamerStatus> = Vec::with_capacity(logins.len());
        let mut known: Vec<TwitchUser> = Vec::new();
        for login in &logins {
            match users.get(login) {
                Some(user) => known.push(user.clone()),
                None => entries.push(unknown_login_status(login)),
            }
        }

        // History lookups run concurrently; one failure affects only that streamer.
        let offset = config.offset_hours;
        let fetched: Vec<(TwitchUser, Result<Vec<VodInfo>, String>)> =
            stream::iter(known.into_iter().map(|user| async move {
                let history = self.fetch_vod_history(&user.id, offset).await;
                (user, history)
            }))
            .buffer_unordered(MAX_CONCURRENT_VOD_REQUESTS)
            .collect()
            .await;

        // Fresh classification instant, after every request has completed.
        let now = Utc::now();

        for (user, history) in fetched {
            match history {
                Ok(vods) if !vods.is_empty() => {
                    entries.push(history_status(&user, &vods, now, offset))
                }
                Ok(_) => entries.push(user_status(&user, StreamerState::Novod)),
                Err(error) => {
                    let mut status = user_status(&user, StreamerState::Error);
                    status.error = Some(error);
                    entries.push(status);
                }
            }
        }

        entries.sort_by(|a, b| {
            state_rank(a.state).cmp(&state_rank(b.state)).then_with(|| {
                if a.state == StreamerState::Upcoming {
                    a.sim_start
                        .cmp(&b.sim_start)
                        .then_with(|| a.login.cmp(&b.login))
                } else {
                    a.login.cmp(&b.login)
                }
            })
        });

        Ok((entries, now))
    }
}

/// Fetch the statuses for `config` using `credentials` (token cache shared
/// process-wide so auto-refresh does not request a token every minute).
pub async fn fetch_statuses(
    config: &AppConfig,
    credentials: &Credentials,
) -> Result<(Vec<StreamerStatus>, DateTime<Utc>), String> {
    shared_client(credentials)?.statuses(config).await
}

/// Documented non-interactive embed URL for a VOD at a join offset.
///
/// Per Twitch's embed documentation the `video` parameter must carry a `v`
/// prefix and `parent` is required. There is **no documented `controls`
/// parameter** — the player keeps Twitch's own controls; this app never
/// promised otherwise (BUG_REVIEW #4). Used as the wrapper's fallback; the
/// primary path is the interactive `Twitch.Player` API in `player.html`.
pub fn player_url(vod_id: &str, join_seconds: u64) -> String {
    let id = vod_id.trim();
    let prefixed = if id.starts_with('v') {
        id.to_string()
    } else {
        format!("v{id}")
    };
    format!(
        "{PLAYER_BASE}?video={prefixed}&parent={PLAYER_PARENT}&autoplay=true&time={}",
        format_timestamp(join_seconds)
    )
}

/// Twitch's player timestamp format: `{h}h{m}m{s}s`.
pub fn format_timestamp(total_seconds: u64) -> String {
    let hours = total_seconds / 3_600;
    let minutes = (total_seconds % 3_600) / 60;
    let seconds = total_seconds % 60;
    format!("{hours}h{minutes}m{seconds}s")
}

fn shared_client(credentials: &Credentials) -> Result<Arc<TwitchClient>, String> {
    static SHARED: OnceLock<StdMutex<Option<Arc<TwitchClient>>>> = OnceLock::new();
    let slot = SHARED.get_or_init(|| StdMutex::new(None));
    client_for(slot, credentials)
}

/// Reuse the cached client only when the *full* credentials match — a rotated
/// secret with the same client ID must rebuild the client (BUG_REVIEW #10).
fn client_for(
    slot: &StdMutex<Option<Arc<TwitchClient>>>,
    credentials: &Credentials,
) -> Result<Arc<TwitchClient>, String> {
    let mut guard = slot
        .lock()
        .map_err(|_| "The Twitch client cache is unusable.".to_string())?;

    if let Some(existing) = guard.as_ref() {
        if existing.credentials == *credentials {
            return Ok(Arc::clone(existing));
        }
    }

    let client = Arc::new(TwitchClient::new(credentials)?);
    *guard = Some(Arc::clone(&client));
    Ok(client)
}

/// Convert one raw Helix video into a [`VodInfo`].
fn vod_from_raw(raw: RawVideo) -> Result<VodInfo, String> {
    let created_at = DateTime::parse_from_rfc3339(&raw.created_at)
        .map_err(|_| {
            format!(
                "Twitch returned a VOD with an unreadable start time (\"{}\").",
                raw.created_at
            )
        })?
        .with_timezone(&Utc);

    let duration_seconds = parse_duration(&raw.duration).ok_or_else(|| {
        format!(
            "Twitch returned a VOD with an unreadable duration (\"{}\").",
            raw.duration
        )
    })?;

    let vod_id = raw.id;
    let url = raw
        .url
        .filter(|url| !url.trim().is_empty())
        .unwrap_or_else(|| format!("{VOD_WEB_BASE}/{vod_id}"));

    Ok(VodInfo {
        title: raw.title.unwrap_or_default(),
        thumbnail_url: raw.thumbnail_url.filter(|url| !url.trim().is_empty()),
        created_at: created_at.to_rfc3339_opts(SecondsFormat::Secs, true),
        duration_seconds,
        url,
        id: vod_id,
    })
}

/// Build a streamer's status by selecting the relevant broadcast from their
/// fetched history (active window first — see `timeshift::select_window`).
fn history_status(
    user: &TwitchUser,
    vods: &[VodInfo],
    now: DateTime<Utc>,
    offset_hours: f64,
) -> StreamerStatus {
    let candidates: Vec<(DateTime<Utc>, u64)> = vods
        .iter()
        .filter_map(|vod| {
            DateTime::parse_from_rfc3339(&vod.created_at)
                .ok()
                .map(|dt| (dt.with_timezone(&Utc), vod.duration_seconds))
        })
        .collect();

    // Candidates preserve history order (newest first); map the chosen
    // candidate back to its VOD by matching start + duration.
    let Some(selection) = timeshift::select_window(&candidates, now, offset_hours) else {
        return user_status(user, StreamerState::Novod);
    };
    let (chosen_start, chosen_duration) = candidates[selection.index];
    let Some(vod) = vods.iter().find(|vod| {
        vod.duration_seconds == chosen_duration
            && DateTime::parse_from_rfc3339(&vod.created_at)
                .map(|dt| dt.with_timezone(&Utc) == chosen_start)
                .unwrap_or(false)
    }) else {
        return user_status(user, StreamerState::Novod);
    };

    vod_status(user, vod.clone(), now, offset_hours)
}

fn normalize_login_list(streamers: &[String]) -> Vec<String> {
    let mut seen: HashSet<String> = HashSet::new();
    let mut logins = Vec::with_capacity(streamers.len());
    for raw in streamers {
        let login = raw.trim().trim_start_matches('@').to_ascii_lowercase();
        if !login.is_empty() && seen.insert(login.clone()) {
            logins.push(login);
        }
    }
    logins
}

fn display_name_for(user: &TwitchUser) -> String {
    if user.display_name.trim().is_empty() {
        user.login.clone()
    } else {
        user.display_name.clone()
    }
}

fn user_status(user: &TwitchUser, state: StreamerState) -> StreamerStatus {
    StreamerStatus {
        login: user.login.clone(),
        display_name: display_name_for(user),
        avatar_url: user.profile_image_url.clone(),
        state,
        vod: None,
        sim_start: None,
        sim_end: None,
        join_seconds: None,
        starts_in_seconds: None,
        progress: None,
        error: None,
    }
}

fn unknown_login_status(login: &str) -> StreamerStatus {
    StreamerStatus {
        login: login.to_string(),
        display_name: login.to_string(),
        avatar_url: None,
        state: StreamerState::Notfound,
        vod: None,
        sim_start: None,
        sim_end: None,
        join_seconds: None,
        starts_in_seconds: None,
        progress: None,
        error: None,
    }
}

fn all_errors(logins: &[String], error: &str) -> Vec<StreamerStatus> {
    let mut entries: Vec<StreamerStatus> = logins
        .iter()
        .map(|login| {
            let mut status = unknown_login_status(login);
            status.state = StreamerState::Error;
            status.error = Some(error.to_string());
            status
        })
        .collect();
    entries.sort_by(|a, b| a.login.cmp(&b.login));
    entries
}

fn vod_status(
    user: &TwitchUser,
    vod: VodInfo,
    now: DateTime<Utc>,
    offset_hours: f64,
) -> StreamerStatus {
    let Ok(created_at) =
        DateTime::parse_from_rfc3339(&vod.created_at).map(|dt| dt.with_timezone(&Utc))
    else {
        let mut status = user_status(user, StreamerState::Error);
        status.error = Some("The VOD's start time could not be parsed.".to_string());
        return status;
    };

    let Evaluation {
        state,
        sim_start,
        sim_end,
        join_seconds,
        starts_in_seconds,
        progress,
    } = timeshift::evaluate(now, created_at, vod.duration_seconds, offset_hours);

    let mut status = user_status(user, map_state(state));
    status.vod = Some(vod);
    status.sim_start = Some(format_instant(sim_start));
    status.sim_end = Some(format_instant(sim_end));
    status.join_seconds = join_seconds;
    status.starts_in_seconds = starts_in_seconds;
    status.progress = progress;
    status
}

fn map_state(state: TimeshiftState) -> StreamerState {
    match state {
        TimeshiftState::Live => StreamerState::Live,
        TimeshiftState::Upcoming => StreamerState::Upcoming,
        TimeshiftState::Offline => StreamerState::Offline,
    }
}

fn state_rank(state: StreamerState) -> u8 {
    match state {
        StreamerState::Live => 0,
        StreamerState::Upcoming => 1,
        StreamerState::Offline => 2,
        StreamerState::Novod => 3,
        StreamerState::Notfound => 4,
        StreamerState::Error => 5,
    }
}

fn format_instant(instant: DateTime<Utc>) -> String {
    instant.to_rfc3339_opts(SecondsFormat::Secs, true)
}

fn body_preview(body: &str) -> String {
    let trimmed = body.trim();
    if trimmed.is_empty() {
        return String::new();
    }
    let preview: String = trimmed.chars().take(MAX_ERROR_BODY_CHARS).collect();
    format!(" — {preview}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn timestamps_are_twitch_formatted() {
        let table: &[(u64, &str)] = &[
            (0, "0h0m0s"),
            (1, "0h0m1s"),
            (59, "0h0m59s"),
            (60, "0h1m0s"),
            (90, "0h1m30s"),
            (3_599, "0h59m59s"),
            (3_600, "1h0m0s"),
            (3_661, "1h1m1s"),
            (86_400, "24h0m0s"),
            (359_999, "99h59m59s"),
        ];
        for (seconds, expected) in table {
            assert_eq!(format_timestamp(*seconds), *expected, "seconds: {seconds}");
        }
    }

    #[test]
    fn player_url_uses_documented_v_prefix() {
        // Twitch embed docs: the video parameter must have a "v" prefix.
        assert_eq!(
            player_url("123456789", 90),
            "https://player.twitch.tv/?video=v123456789&parent=localhost&autoplay=true&time=0h1m30s"
        );
        // Already-prefixed ids and whitespace are handled.
        assert_eq!(
            player_url("  v42  ", 7_381),
            "https://player.twitch.tv/?video=v42&parent=localhost&autoplay=true&time=2h3m1s"
        );
    }

    #[test]
    fn player_url_has_no_undocumented_controls_param() {
        // There is no documented `controls` embed parameter; claiming the
        // player is locked via URL is not supported (BUG_REVIEW #4).
        let url = player_url("1", 0);
        assert!(!url.contains("controls"), "url: {url}");
        assert!(url.contains("autoplay=true"));
        assert!(url.ends_with("&time=0h0m0s"));
    }

    #[test]
    fn token_body_parsing_is_explicit() {
        let (token, ttl) =
            parse_token_body(r#"{"access_token":"tok","expires_in":3600,"token_type":"bearer"}"#)
                .expect("valid body");
        assert_eq!(token, "tok");
        assert_eq!(ttl, Duration::from_secs(3_600));

        assert!(
            parse_token_body(r#"{"expires_in":3600}"#).is_err(),
            "missing token"
        );
        assert!(
            parse_token_body(r#"{"access_token":" ","expires_in":3600}"#).is_err(),
            "blank token"
        );
        assert!(
            parse_token_body(r#"{"access_token":"tok"}"#).is_err(),
            "missing expiry"
        );
        assert!(
            parse_token_body(r#"{"access_token":"tok","expires_in":0}"#).is_err(),
            "zero expiry"
        );
        assert!(parse_token_body("not json").is_err(), "garbage");
    }

    #[test]
    fn token_lifetimes_never_exceed_true_expiry() {
        let (usable, hard) = token_lifetimes(Duration::from_secs(3_600));
        assert_eq!(hard, Duration::from_secs(3_600));
        assert_eq!(usable, Duration::from_secs(3_540), "60s early refresh");

        // Short-lived tokens: previously floored to 30s+ (past real expiry).
        let (usable, hard) = token_lifetimes(Duration::from_secs(20));
        assert_eq!(hard, Duration::from_secs(20));
        assert_eq!(usable, Duration::from_secs(15));
        assert!(usable < hard);
    }

    #[tokio::test]
    async fn late_401_does_not_clear_refreshed_token() {
        let credentials = Credentials {
            client_id: "test-id".to_string(),
            client_secret: "test-secret".to_string(),
        };
        let client = TwitchClient::new(&credentials).expect("client builds");
        let now = Instant::now();
        *client.token.lock().await = Some(CachedToken {
            value: "new-token".to_string(),
            refresh_after: now + Duration::from_secs(60),
            expires_at: now + Duration::from_secs(120),
        });

        // A request that used the OLD token gets a late 401: cache survives.
        client.invalidate_token("old-token").await;
        assert_eq!(
            client.token.lock().await.as_ref().map(|t| t.value.as_str()),
            Some("new-token")
        );

        // Invalidating the token actually in use does clear it.
        client.invalidate_token("new-token").await;
        assert!(client.token.lock().await.is_none());
    }

    #[test]
    fn client_identity_includes_the_secret() {
        let slot: StdMutex<Option<Arc<TwitchClient>>> = StdMutex::new(None);
        let base = Credentials {
            client_id: "same-id".to_string(),
            client_secret: "secret-one".to_string(),
        };
        let rotated = Credentials {
            client_secret: "secret-two".to_string(),
            ..base.clone()
        };

        let first = client_for(&slot, &base).expect("first client");
        let reused = client_for(&slot, &base).expect("reused client");
        assert!(Arc::ptr_eq(&first, &reused), "identical credentials reuse");

        let rebuilt = client_for(&slot, &rotated).expect("rebuilt client");
        assert!(
            !Arc::ptr_eq(&first, &rebuilt),
            "rotated secret with same ID must rebuild (BUG_REVIEW #10)"
        );
    }

    #[test]
    fn history_selection_keeps_active_replay() {
        // BUG_REVIEW #1 regression at the history level: yesterday's 4h
        // 18:00 stream at +24h is live today 18:00-22:00, even though a
        // newer VOD exists (newest first, as Helix returns them).
        let user = TwitchUser {
            id: "1".to_string(),
            login: "squeex".to_string(),
            display_name: "Squeex".to_string(),
            profile_image_url: None,
        };
        let mk = |id: &str, created: &str, duration_seconds: u64| VodInfo {
            id: id.to_string(),
            title: format!("VOD {id}"),
            url: format!("https://www.twitch.tv/videos/{id}"),
            thumbnail_url: None,
            created_at: created.to_string(),
            duration_seconds,
        };
        let vods = vec![
            mk("new", "2024-03-02T19:00:00Z", 3 * 3_600),
            mk("yesterday", "2024-03-01T18:00:00Z", 4 * 3_600),
        ];
        let now = DateTime::parse_from_rfc3339("2024-03-02T19:30:00Z")
            .unwrap()
            .with_timezone(&Utc);
        let status = history_status(&user, &vods, now, 24.0);
        assert_eq!(status.state, StreamerState::Live);
        assert_eq!(
            status.vod.as_ref().map(|v| v.id.as_str()),
            Some("yesterday")
        );
        assert_eq!(status.join_seconds, Some(5_400));
    }

    #[test]
    fn duration_parsing_is_available_from_this_module() {
        assert_eq!(parse_duration("3h25m10s"), Some(12_310));
        assert_eq!(parse_duration("45m"), Some(2_700));
        assert_eq!(parse_duration("90s"), Some(90));
        assert_eq!(parse_duration("1h0m0s"), Some(3_600));
        assert_eq!(parse_duration("garbage"), None);
        assert_eq!(parse_duration(""), None);
    }

    #[test]
    fn state_ranking_matches_spec_order() {
        let ordered = [
            StreamerState::Live,
            StreamerState::Upcoming,
            StreamerState::Offline,
            StreamerState::Novod,
            StreamerState::Notfound,
            StreamerState::Error,
        ];
        let ranks: Vec<u8> = ordered.iter().copied().map(state_rank).collect();
        assert_eq!(ranks, vec![0, 1, 2, 3, 4, 5]);
    }

    #[test]
    fn unknown_logins_are_reported_as_notfound() {
        let status = unknown_login_status("nobody_here");
        assert_eq!(status.state, StreamerState::Notfound);
        assert_eq!(status.login, "nobody_here");
        assert_eq!(status.display_name, "nobody_here");
        assert!(status.vod.is_none());
        assert!(status.error.is_none());
    }

    #[test]
    fn batch_failures_degrade_to_error_entries() {
        let logins = vec!["zeta".to_string(), "alpha".to_string()];
        let entries = all_errors(&logins, "Twitch request failed");

        assert_eq!(entries.len(), 2);
        assert!(entries.iter().all(|e| e.state == StreamerState::Error));
        assert!(entries
            .iter()
            .all(|e| e.error.as_deref() == Some("Twitch request failed")));
        // Error entries are still sorted alphabetically by login.
        assert_eq!(entries[0].login, "alpha");
        assert_eq!(entries[1].login, "zeta");
    }

    #[test]
    fn vod_status_uses_timeshift_windows() {
        let user = TwitchUser {
            id: "1".to_string(),
            login: "squeex".to_string(),
            display_name: "Squeex".to_string(),
            profile_image_url: Some("https://example.invalid/avatar.png".to_string()),
        };
        let vod = VodInfo {
            id: "v1".to_string(),
            title: "A VOD".to_string(),
            url: "https://www.twitch.tv/videos/v1".to_string(),
            thumbnail_url: None,
            created_at: "2024-03-01T12:00:00Z".to_string(),
            duration_seconds: 3_600,
        };

        let start = DateTime::parse_from_rfc3339("2024-03-02T12:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        let live = vod_status(&user, vod.clone(), start, 24.0);
        assert_eq!(live.state, StreamerState::Live);
        assert_eq!(live.join_seconds, Some(0));
        assert_eq!(live.sim_start.as_deref(), Some("2024-03-02T12:00:00Z"));
        assert_eq!(live.sim_end.as_deref(), Some("2024-03-02T13:00:00Z"));
        assert_eq!(live.avatar_url, user.profile_image_url);

        let upcoming = vod_status(&user, vod.clone(), start - chrono::Duration::hours(1), 24.0);
        assert_eq!(upcoming.state, StreamerState::Upcoming);
        assert_eq!(upcoming.starts_in_seconds, Some(3_600));

        let offline = vod_status(&user, vod, start + chrono::Duration::hours(1), 24.0);
        assert_eq!(offline.state, StreamerState::Offline);
        assert!(offline.join_seconds.is_none());
    }

    #[test]
    fn login_lists_are_normalized_and_deduped() {
        let logins = normalize_login_list(&[
            " Squeex ".to_string(),
            "@ludwig".to_string(),
            "LUDWIG".to_string(),
            "".to_string(),
            "squeex".to_string(),
        ]);
        assert_eq!(logins, vec!["squeex", "ludwig"]);
    }

    #[test]
    fn error_body_preview_is_trimmed() {
        assert_eq!(body_preview("   "), "");
        assert_eq!(body_preview(" oops "), " — oops");
        let long = "x".repeat(500);
        assert_eq!(
            body_preview(&long).chars().count(),
            3 + MAX_ERROR_BODY_CHARS
        );
    }
}

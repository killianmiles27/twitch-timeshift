//! Config load / save / validation on disk.
//!
//! The config lives in `app_config_dir()/config.json` and is always written
//! atomically (temp file + rename) as pretty JSON. Loading never panics: a
//! missing, unreadable or corrupt file falls back to [`default_config`].
//!
//! Logins are normalized to Twitch's canonical form (lowercase, optional `@`
//! stripped, `^[a-z0-9_]{3,25}$`) and deduped; the offset is clamped to
//! `0..=168` hours and the refresh interval to `0..=3600` seconds. Only an
//! explicitly provided, unknown IANA timezone is a hard error.

use std::collections::HashSet;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use chrono_tz::{Tz, TZ_VARIANTS};

use crate::models::{AppConfig, ConfigLoad};
use crate::timeshift::MAX_OFFSET_HOURS;

/// File name of the persisted config inside the app config directory.
pub const CONFIG_FILE_NAME: &str = "config.json";
/// Used when the system timezone cannot be detected or is unknown to chrono-tz.
pub const FALLBACK_TIMEZONE: &str = "Europe/Berlin";
pub const DEFAULT_OFFSET_HOURS: f64 = 24.0;
pub const DEFAULT_AUTO_REFRESH_SECONDS: u32 = 60;
pub const MAX_AUTO_REFRESH_SECONDS: u32 = 3_600;
pub const MIN_LOGIN_LENGTH: usize = 3;
pub const MAX_LOGIN_LENGTH: usize = 25;

/// New installations start without anyone else's personal follows.
pub fn default_streamers() -> Vec<String> {
    Vec::new()
}

/// A marker beside the executable makes settings travel with a portable app.
pub fn portable_config_dir(exe: &Path) -> Option<PathBuf> {
    let dir = exe.parent()?;
    dir.join("portable.txt").is_file().then(|| dir.join("data"))
}

/// Parse an IANA timezone name, tolerating different casing.
pub fn parse_timezone(name: &str) -> Option<Tz> {
    let trimmed = name.trim();
    if trimmed.is_empty() {
        return None;
    }
    if let Ok(tz) = trimmed.parse::<Tz>() {
        return Some(tz);
    }
    TZ_VARIANTS
        .iter()
        .copied()
        .find(|tz| tz.name().eq_ignore_ascii_case(trimmed))
}

/// The system timezone name, falling back to [`FALLBACK_TIMEZONE`].
///
/// `iana-time-zone` 0.1.65 exposes `get_timezone() -> Result<String, _>`
/// (there is no `local_timezone()`), which already returns an IANA name.
pub fn system_timezone() -> String {
    iana_time_zone::get_timezone()
        .ok()
        .and_then(|name| parse_timezone(&name).map(|tz| tz.name().to_string()))
        .unwrap_or_else(|| FALLBACK_TIMEZONE.to_string())
}

/// SPEC.md defaults: migrated streamers, system timezone, 24 h offset, 60 s refresh.
pub fn default_config() -> AppConfig {
    AppConfig {
        streamers: default_streamers(),
        timezone: system_timezone(),
        offset_hours: DEFAULT_OFFSET_HOURS,
        auto_refresh_seconds: DEFAULT_AUTO_REFRESH_SECONDS,
    }
}

/// Whether `login` matches Twitch's `^[a-z0-9_]{3,25}$`.
pub fn is_valid_login(login: &str) -> bool {
    (MIN_LOGIN_LENGTH..=MAX_LOGIN_LENGTH).contains(&login.len())
        && login
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
}

/// Lowercase, strip a single leading `@` and validate a login.
pub fn normalize_login(raw: &str) -> Option<String> {
    let trimmed = raw.trim();
    let without_at = trimmed.strip_prefix('@').unwrap_or(trimmed);
    let login = without_at.trim().to_ascii_lowercase();
    if is_valid_login(&login) {
        Some(login)
    } else {
        None
    }
}

/// Normalize and dedupe a streamer list, preserving the caller's order.
pub fn normalize_streamers(streamers: &[String]) -> Vec<String> {
    let mut seen: HashSet<String> = HashSet::new();
    let mut normalized = Vec::with_capacity(streamers.len());
    for raw in streamers {
        if let Some(login) = normalize_login(raw) {
            if seen.insert(login.clone()) {
                normalized.push(login);
            }
        }
    }
    normalized
}

/// Canonical timezone name, or the system/default timezone when unusable.
pub fn normalize_timezone(name: &str) -> String {
    parse_timezone(name)
        .map(|tz| tz.name().to_string())
        .unwrap_or_else(system_timezone)
}

/// Clamp the offset to `0..=168`, replacing non-finite values with the default.
pub fn normalize_offset_hours(hours: f64) -> f64 {
    if hours.is_finite() {
        hours.clamp(0.0, MAX_OFFSET_HOURS)
    } else {
        DEFAULT_OFFSET_HOURS
    }
}

/// Clamp the auto-refresh interval to `0..=3600` (`0` disables refreshing).
pub fn normalize_auto_refresh_seconds(seconds: u32) -> u32 {
    seconds.min(MAX_AUTO_REFRESH_SECONDS)
}

/// Normalize every field of a config (never fails).
pub fn normalize_config(config: AppConfig) -> AppConfig {
    AppConfig {
        streamers: normalize_streamers(&config.streamers),
        timezone: normalize_timezone(&config.timezone),
        offset_hours: normalize_offset_hours(config.offset_hours),
        auto_refresh_seconds: normalize_auto_refresh_seconds(config.auto_refresh_seconds),
    }
}

/// Reject configs that cannot be normalized into something usable.
///
/// Logins are filtered and numbers are clamped, so the only hard error is an
/// explicitly provided timezone that is not a known IANA zone.
pub fn validate_config(config: &AppConfig) -> Result<(), String> {
    let timezone = config.timezone.trim();
    if !timezone.is_empty() && parse_timezone(timezone).is_none() {
        let shown: String = timezone.chars().take(64).collect();
        return Err(format!(
            "Unknown timezone \"{shown}\". Use an IANA timezone name such as \"Europe/Berlin\"."
        ));
    }
    if !config.offset_hours.is_finite() {
        return Err("The timeshift offset must be a finite number of hours.".to_string());
    }
    Ok(())
}

/// Path of the persisted config file.
pub fn config_path(config_dir: &Path) -> PathBuf {
    config_dir.join(CONFIG_FILE_NAME)
}

/// Load the config for internal use.
pub fn load_config(config_dir: &Path) -> AppConfig {
    load_config_detailed(config_dir).config
}

/// Load the config, distinguishing first launch, healthy loads, migration of
/// partial/older files, and corrupt data (BUG_REVIEW #12).
///
/// A corrupt or unreadable existing file is copied to `config.json.bak`
/// (best effort) and reported via `warning`; the original file is left in
/// place until the next successful save, so nothing is silently destroyed.
pub fn load_config_detailed(config_dir: &Path) -> ConfigLoad {
    let path = config_path(config_dir);
    let text = match fs::read_to_string(&path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return ConfigLoad {
                config: default_config(),
                first_launch: true,
                recovered: false,
                warning: None,
            };
        }
        Err(error) => {
            return ConfigLoad {
                config: default_config(),
                first_launch: false,
                recovered: true,
                warning: Some(format!(
                    "Your settings file could not be read ({error}). Defaults are shown; \
                     your streamers were not changed on disk."
                )),
            };
        }
    };

    let value: serde_json::Value =
        match serde_json::from_str::<serde_json::Value>(text.trim_start_matches('\u{feff}')) {
            Ok(value) if value.is_object() => value,
            _ => {
                let backup = backup_corrupt_file(config_dir);
                return ConfigLoad {
                    config: default_config(),
                    first_launch: false,
                    recovered: true,
                    warning: Some(format!(
                    "Your settings file was corrupt and could not be used. Defaults are shown. \
                     The original was preserved as {backup}."
                )),
                };
            }
        };

    ConfigLoad {
        config: migrate_config(&value),
        first_launch: false,
        recovered: false,
        warning: None,
    }
}

/// Per-field migration: every present field is honored, every missing or
/// mistyped field falls back individually — one absent setting never discards
/// the user's streamer list (BUG_REVIEW #12).
pub fn migrate_config(value: &serde_json::Value) -> AppConfig {
    let defaults = default_config();
    let streamers = value
        .get("streamers")
        .and_then(|v| v.as_array())
        .map(|items| {
            normalize_streamers(
                &items
                    .iter()
                    .filter_map(|item| item.as_str().map(str::to_string))
                    .collect::<Vec<_>>(),
            )
        })
        .unwrap_or(defaults.streamers);
    let timezone = value
        .get("timezone")
        .and_then(|v| v.as_str())
        .map(normalize_timezone)
        .unwrap_or(defaults.timezone);
    let offset_hours = value
        .get("offsetHours")
        .and_then(|v| v.as_f64())
        .map(normalize_offset_hours)
        .unwrap_or(defaults.offset_hours);
    let auto_refresh_seconds = value
        .get("autoRefreshSeconds")
        .and_then(|v| v.as_u64())
        .map(|v| normalize_auto_refresh_seconds(u32::try_from(v).unwrap_or(u32::MAX)))
        .unwrap_or(defaults.auto_refresh_seconds);

    AppConfig {
        streamers,
        timezone,
        offset_hours,
        auto_refresh_seconds,
    }
}

/// Copy a corrupt config to `config.json.bak`; returns a display name.
fn backup_corrupt_file(config_dir: &Path) -> String {
    let backup_path = config_dir.join(format!("{CONFIG_FILE_NAME}.bak"));
    match fs::copy(config_path(config_dir), &backup_path) {
        Ok(_) => backup_path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "config.json.bak".to_string()),
        Err(_) => "a backup could not be written".to_string(),
    }
}

/// Validate + normalize + persist the config atomically; returns what was stored.
pub fn save_config(config_dir: &Path, config: &AppConfig) -> Result<AppConfig, String> {
    validate_config(config)?;
    let normalized = normalize_config(config.clone());

    fs::create_dir_all(config_dir).map_err(|error| {
        format!(
            "Could not create the config directory \"{}\": {error}",
            config_dir.display()
        )
    })?;

    let json = serde_json::to_string_pretty(&normalized)
        .map_err(|error| format!("Could not serialize the config: {error}"))?;

    let path = config_path(config_dir);
    let temp_path = config_dir.join(format!(".{}.{}.tmp", CONFIG_FILE_NAME, std::process::id()));

    let write_result = (|| -> std::io::Result<()> {
        let mut file = fs::File::create(&temp_path)?;
        file.write_all(json.as_bytes())?;
        file.write_all(b"\n")?;
        file.sync_all()
    })();
    if let Err(error) = write_result {
        let _ = fs::remove_file(&temp_path);
        return Err(format!(
            "Could not write the config file \"{}\": {error}",
            temp_path.display()
        ));
    }

    if let Err(error) = fs::rename(&temp_path, &path) {
        let _ = fs::remove_file(&temp_path);
        return Err(format!(
            "Could not replace the config file \"{}\": {error}",
            path.display()
        ));
    }

    Ok(normalized)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn portable_marker_selects_data_beside_executable() {
        let dir = test_dir("portable");
        fs::create_dir_all(&dir).unwrap();
        let exe = dir.join("app.exe");
        assert_eq!(portable_config_dir(&exe), None);
        fs::write(dir.join("portable.txt"), "portable").unwrap();
        assert_eq!(portable_config_dir(&exe), Some(dir.join("data")));
        cleanup(&dir);
    }

    #[test]
    fn windows_utf8_bom_settings_are_accepted() {
        let dir = test_dir("bom");
        fs::create_dir_all(&dir).unwrap();
        fs::write(config_path(&dir), "\u{feff}{\"streamers\":[\"example\"]}").unwrap();
        let loaded = load_config_detailed(&dir);
        assert!(!loaded.recovered);
        assert_eq!(loaded.config.streamers, vec!["example"]);
        cleanup(&dir);
    }

    fn test_dir(tag: &str) -> PathBuf {
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or_default();
        std::env::temp_dir().join(format!(
            "twitch-timeshift-test-{tag}-{}-{unique}",
            std::process::id()
        ))
    }

    /// Remove the files this test dir may contain, then the dir itself.
    fn cleanup(dir: &Path) {
        let _ = fs::remove_file(config_path(dir));
        if let Ok(entries) = fs::read_dir(dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_file() {
                    let _ = fs::remove_file(path);
                }
            }
        }
        let _ = fs::remove_dir(dir);
    }

    fn config_with(streamers: &[&str], timezone: &str, offset: f64, refresh: u32) -> AppConfig {
        AppConfig {
            streamers: streamers.iter().map(|s| (*s).to_string()).collect(),
            timezone: timezone.to_string(),
            offset_hours: offset,
            auto_refresh_seconds: refresh,
        }
    }

    #[test]
    fn logins_are_lowercased_deduped_and_filtered() {
        let normalized = normalize_streamers(&[
            "Squeex".to_string(),
            "  squeex  ".to_string(),
            "@Ludwig".to_string(),
            "LUDWIG".to_string(),
            "hasan_abi".to_string(),
            "ab".to_string(),
            "bad login".to_string(),
            "".to_string(),
            "@@slimeirl".to_string(),
        ]);

        assert_eq!(normalized, vec!["squeex", "ludwig", "hasan_abi"]);
    }

    #[test]
    fn login_length_bounds() {
        assert!(is_valid_login("abc"));
        assert!(is_valid_login("a234567890123456789012345"));
        assert!(!is_valid_login("ab"));
        assert!(!is_valid_login("a2345678901234567890123456"));
        assert!(!is_valid_login("Abc"));
        assert!(!is_valid_login("has-an"));
    }

    #[test]
    fn bad_timezone_is_rejected() {
        let dir = test_dir("bad-tz");
        let config = config_with(&["squeex"], "Mars/Olympus_Mons", 24.0, 60);

        let result = save_config(&dir, &config);
        assert!(result.is_err(), "unknown timezone must be rejected");
        assert!(!config_path(&dir).exists(), "nothing should be persisted");

        cleanup(&dir);
    }

    #[test]
    fn timezone_is_canonicalized_and_case_insensitive() {
        assert_eq!(
            parse_timezone("europe/berlin").map(|tz| tz.name()),
            Some("Europe/Berlin")
        );
        assert_eq!(normalize_timezone("EUROPE/BERLIN"), "Europe/Berlin");
        assert_eq!(normalize_timezone("UTC"), "UTC");
        assert!(parse_timezone("Not/AZone").is_none());
        assert!(parse_timezone("  ").is_none());
    }

    #[test]
    fn values_are_clamped() {
        assert_eq!(normalize_offset_hours(-5.0), 0.0);
        assert_eq!(normalize_offset_hours(0.0), 0.0);
        assert_eq!(normalize_offset_hours(42.5), 42.5);
        assert_eq!(normalize_offset_hours(500.0), MAX_OFFSET_HOURS);
        assert_eq!(normalize_offset_hours(f64::INFINITY), DEFAULT_OFFSET_HOURS);
        assert_eq!(normalize_offset_hours(f64::NAN), DEFAULT_OFFSET_HOURS);
        assert_eq!(normalize_auto_refresh_seconds(60), 60);
        assert_eq!(normalize_auto_refresh_seconds(0), 0);
        assert_eq!(
            normalize_auto_refresh_seconds(99_999),
            MAX_AUTO_REFRESH_SECONDS
        );
    }

    #[test]
    fn default_config_matches_spec() {
        let config = default_config();
        assert_eq!(config.streamers, default_streamers());
        assert!(config.streamers.is_empty());
        assert!(config.streamers.iter().all(|login| is_valid_login(login)));
        assert_eq!(config.offset_hours, 24.0);
        assert_eq!(config.auto_refresh_seconds, 60);
        assert!(parse_timezone(&config.timezone).is_some());
    }

    #[test]
    fn missing_and_corrupt_files_load_defaults() {
        let dir = test_dir("missing");
        assert_eq!(load_config(&dir), default_config());

        fs::create_dir_all(&dir).unwrap();
        fs::write(config_path(&dir), b"{ this is not json").unwrap();
        assert_eq!(load_config(&dir), default_config());

        fs::write(config_path(&dir), b"{\"streamers\":42}").unwrap();
        assert_eq!(load_config(&dir), default_config());

        cleanup(&dir);
    }

    #[test]
    fn partial_config_migrates_without_losing_streamers() {
        // BUG_REVIEW #12 regression: an older config missing newer fields
        // must keep the user's list.
        let dir = test_dir("migrate");
        fs::create_dir_all(&dir).unwrap();
        fs::write(
            config_path(&dir),
            br#"{"streamers":["MyStreamer","other_one"],"timezone":"America/New_York"}"#,
        )
        .unwrap();

        let load = load_config_detailed(&dir);
        assert!(!load.first_launch);
        assert!(!load.recovered);
        assert_eq!(load.config.streamers, vec!["mystreamer", "other_one"]);
        assert_eq!(load.config.timezone, "America/New_York");
        assert_eq!(load.config.offset_hours, DEFAULT_OFFSET_HOURS);
        assert_eq!(
            load.config.auto_refresh_seconds,
            DEFAULT_AUTO_REFRESH_SECONDS
        );

        cleanup(&dir);
    }

    #[test]
    fn corrupt_config_is_backed_up_and_reported() {
        let dir = test_dir("corrupt");
        fs::create_dir_all(&dir).unwrap();
        fs::write(config_path(&dir), b"{ not json !!").unwrap();

        let load = load_config_detailed(&dir);
        assert!(!load.first_launch);
        assert!(load.recovered, "corruption must be reported, not silent");
        assert!(load.warning.is_some());
        // Original preserved as backup; untouched original remains in place.
        let backup = dir.join(format!("{CONFIG_FILE_NAME}.bak"));
        assert_eq!(fs::read(&backup).unwrap(), b"{ not json !!");
        assert_eq!(
            fs::read(config_path(&dir)).unwrap(),
            b"{ not json !!",
            "load must not overwrite the recoverable original"
        );

        cleanup(&dir);
        let _ = fs::remove_file(backup);
    }

    #[test]
    fn first_launch_is_distinguished() {
        let dir = test_dir("first-launch");
        let load = load_config_detailed(&dir);
        assert!(load.first_launch);
        assert!(!load.recovered);
        assert!(load.warning.is_none());
        cleanup(&dir);
    }

    #[test]
    fn replacement_save_overwrites_existing_file() {
        // Windows rename-replace semantics: saving over an existing
        // config.json must succeed, not only the first write (BUG_REVIEW #5).
        let dir = test_dir("replace");
        let first = config_with(&["squeex"], "UTC", 24.0, 60);
        save_config(&dir, &first).expect("first save");

        let second = config_with(&["ludwig"], "Europe/Berlin", 48.0, 0);
        let saved = save_config(&dir, &second).expect("replacement save on Windows");
        assert_eq!(load_config(&dir), saved);
        assert_eq!(saved.streamers, vec!["ludwig"]);

        cleanup(&dir);
    }

    #[test]
    fn save_then_load_roundtrip() {
        let dir = test_dir("roundtrip");
        let config = config_with(&["Squeex", "Ludwig", "ludwig"], "Europe/Berlin", 12.75, 90);

        let saved = save_config(&dir, &config).expect("valid config should save");
        assert_eq!(saved.streamers, vec!["squeex", "ludwig"]);
        assert_eq!(saved.offset_hours, 12.75);
        assert_eq!(saved.auto_refresh_seconds, 90);
        assert_eq!(saved.timezone, "Europe/Berlin");

        let loaded = load_config(&dir);
        assert_eq!(loaded, saved);

        // Pretty JSON, and no temp files left behind.
        let text = fs::read_to_string(config_path(&dir)).unwrap();
        assert!(text.contains('\n'), "config should be pretty printed");
        let leftovers: Vec<_> = fs::read_dir(&dir)
            .unwrap()
            .filter_map(|entry| entry.ok())
            .filter(|entry| entry.file_name().to_string_lossy().ends_with(".tmp"))
            .collect();
        assert!(
            leftovers.is_empty(),
            "atomic write must not leave temp files"
        );

        cleanup(&dir);
    }

    #[test]
    fn save_clamps_before_persisting() {
        let dir = test_dir("clamp");
        let config = config_with(&["Squeex"], "UTC", 999.0, 7_200);

        let saved = save_config(&dir, &config).expect("valid config should save");
        assert_eq!(saved.offset_hours, MAX_OFFSET_HOURS);
        assert_eq!(saved.auto_refresh_seconds, MAX_AUTO_REFRESH_SECONDS);
        assert_eq!(saved.streamers, vec!["squeex"]);
        assert_eq!(load_config(&dir), saved);

        cleanup(&dir);
    }

    #[test]
    fn empty_timezone_falls_back_to_the_system_zone() {
        let dir = test_dir("empty-tz");
        let config = config_with(&["squeex"], "", 0.0, 60);

        let saved = save_config(&dir, &config).expect("empty timezone is not fatal");
        assert_eq!(saved.timezone, system_timezone());
        assert!(parse_timezone(&saved.timezone).is_some());

        cleanup(&dir);
    }
}

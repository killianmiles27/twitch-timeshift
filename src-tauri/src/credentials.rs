//! Twitch API credential resolution.
//!
//! Resolution order (first hit wins):
//! 1. Environment variables `TWITCH_CLIENT_ID` + `TWITCH_CLIENT_SECRET`
//! 2. A secrets JSON file `{ "clientId": "...", "clientSecret": "..." }`
//!    (snake_case keys also accepted) found in the app config dir, next to
//!    the executable, or in the current working directory (dev).
//! 3. Compile-time `option_env!("TWITCH_CLIENT_ID")` /
//!    `option_env!("TWITCH_CLIENT_SECRET")` (release builds via
//!    `scripts/build-release.ps1`, which loads the gitignored
//!    `.twitch-secrets.json` into the environment without printing it).
//!
//! Values are never logged, returned to the frontend, or put in errors.

use std::path::{Path, PathBuf};

use crate::models::CredentialsStatus;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Credentials {
    pub client_id: String,
    pub client_secret: String,
}

fn non_empty(value: Option<String>) -> Option<String> {
    value.filter(|v| !v.trim().is_empty())
}

fn from_env() -> Option<Credentials> {
    Some(Credentials {
        client_id: non_empty(std::env::var("TWITCH_CLIENT_ID").ok())?,
        client_secret: non_empty(std::env::var("TWITCH_CLIENT_SECRET").ok())?,
    })
}

fn from_compile_time() -> Option<Credentials> {
    Some(Credentials {
        client_id: non_empty(option_env!("TWITCH_CLIENT_ID").map(str::to_string))?,
        client_secret: non_empty(option_env!("TWITCH_CLIENT_SECRET").map(str::to_string))?,
    })
}

fn parse_secrets_json(text: &str) -> Option<Credentials> {
    let value: serde_json::Value = serde_json::from_str(text).ok()?;
    let get = |keys: [&str; 2]| -> Option<String> {
        keys.iter()
            .find_map(|k| value.get(k).and_then(|v| v.as_str()))
            .and_then(|s| non_empty(Some(s.to_string())))
    };
    Some(Credentials {
        client_id: get(["clientId", "client_id"])?,
        client_secret: get(["clientSecret", "client_secret"])?,
    })
}

fn candidate_files(app_config_dir: Option<&Path>) -> Vec<PathBuf> {
    let mut files = Vec::new();
    if let Some(dir) = app_config_dir {
        files.push(dir.join("twitch-secrets.json"));
        files.push(dir.join(".twitch-secrets.json"));
    }
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            files.push(dir.join("twitch-secrets.json"));
            files.push(dir.join(".twitch-secrets.json"));
        }
    }
    files.push(PathBuf::from("twitch-secrets.json"));
    files.push(PathBuf::from(".twitch-secrets.json"));
    files
}

fn from_file(app_config_dir: Option<&Path>) -> Option<Credentials> {
    if let Ok(path) = std::env::var("TWITCH_SECRETS_FILE") {
        if let Ok(text) = std::fs::read_to_string(&path) {
            if let Some(creds) = parse_secrets_json(&text) {
                return Some(creds);
            }
        }
    }
    for path in candidate_files(app_config_dir) {
        if let Ok(text) = std::fs::read_to_string(&path) {
            if let Some(creds) = parse_secrets_json(&text) {
                return Some(creds);
            }
        }
    }
    None
}

/// Which source provides credentials, without exposing the values.
pub fn source(app_config_dir: Option<&Path>) -> &'static str {
    if from_env().is_some() {
        "environment"
    } else if from_file(app_config_dir).is_some() {
        "file"
    } else if from_compile_time().is_some() {
        "embedded"
    } else {
        "missing"
    }
}

pub fn load(app_config_dir: Option<&Path>) -> Option<Credentials> {
    from_env()
        .or_else(|| from_file(app_config_dir))
        .or_else(from_compile_time)
}

pub fn status(app_config_dir: Option<&Path>) -> CredentialsStatus {
    let source = source(app_config_dir);
    CredentialsStatus {
        configured: source != "missing",
        source: source.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::parse_secrets_json;

    #[test]
    fn parses_camel_case_keys() {
        let creds = parse_secrets_json(r#"{"clientId":"abc","clientSecret":"def"}"#)
            .expect("camelCase secrets should parse");
        assert_eq!(creds.client_id, "abc");
        assert_eq!(creds.client_secret, "def");
    }

    #[test]
    fn parses_snake_case_keys() {
        let creds = parse_secrets_json(r#"{"client_id":"abc","client_secret":"def"}"#)
            .expect("snake_case secrets should parse");
        assert_eq!(creds.client_id, "abc");
    }

    #[test]
    fn rejects_missing_or_empty_values() {
        assert!(parse_secrets_json(r#"{"clientId":"abc"}"#).is_none());
        assert!(parse_secrets_json(r#"{"clientId":"","clientSecret":"def"}"#).is_none());
        assert!(parse_secrets_json("not json").is_none());
    }
}

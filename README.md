# Twitch Timeshift

Watch Twitch broadcasts on a delayed, simulated-live schedule. Choose a fixed delay from 0 to 168 hours, add streamers, and open an active replay at its simulated live position. The timezone changes display only. The player keeps Twitch's controls; while playing, drift over 20 seconds is corrected. Pausing is respected.

## Windows portable app

Extract the portable ZIP and double-click **Twitch Timeshift.exe**. Move the entire folder to your Apps folder. A `portable.txt` marker stores settings in `data/config.json` beside the executable. Keep the folder writable. Windows WebView2 Runtime is required.

Public packages contain no credentials, followed streamers, or personal settings. Register your own application at the [Twitch developer console](https://dev.twitch.tv/console), then place `twitch-secrets.json` beside the executable:

```json
{"clientId":"YOUR_CLIENT_ID","clientSecret":"YOUR_CLIENT_SECRET"}
```

Alternatively set `TWITCH_CLIENT_ID` and `TWITCH_CLIENT_SECRET`, or use `TWITCH_SECRETS_FILE`. Credentials are read at runtime and never compiled into the executable. Keep your credentials private. Click Refresh after changing them.

NSIS and MSI installers are also supported. Installed apps without a portable marker store settings in the normal OS app config directory.

## Recovery

- Missing/rejected credentials: correct the file and press Refresh.
- Network failure: the previous schedule stays visible and is marked stale; initial failures offer Retry.
- Corrupt settings: the original is kept as `config.json.bak`; defaults are shown with a warning. Missing fields are migrated individually.
- Playback blocked: Twitch may require a user gesture, login, or access to a restricted VOD. Retry playback or open the video on Twitch.
- Delay changes: open players receive the new schedule; a replay moved outside its window pauses.

## Development and packaging

Requires Node 22+, Rust MSVC, and Windows WebView2.

```powershell
npm ci
npm test
npm run build
cargo test --manifest-path src-tauri/Cargo.toml
cargo clippy --manifest-path src-tauri/Cargo.toml --all-targets -- -D warnings
powershell -ExecutionPolicy Bypass -File scripts/build-release.ps1
powershell -ExecutionPolicy Bypass -File scripts/build-release.ps1 -Installers
```

The output is `release/Twitch Timeshift`. For a private portable folder, put your key in the gitignored `.twitch-secrets.json` and optionally seed settings using `.personal-config.json`, then pass `-Personal`. Only that private output receives these files. Never share it. `-SkipBuild` packages an already-built executable.

Earlier v2.0.0 release binaries embedded credentials. The owner of that registration should rotate its secret in the Twitch console; a new build cannot revoke an old binary's key.

See [SPEC.md](SPEC.md) for scheduling and API details, and [LICENSE](LICENSE).

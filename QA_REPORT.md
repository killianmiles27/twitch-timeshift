# Version 2.0.1 verification

Tested on Windows x64, 2026-11-06.

## Automated checks

- 51 Rust tests: scheduling, history selection, token expiry/rotation, late-401 handling, config migration/recovery/replacement, portable paths, UTF-8 BOM files, and browser URL validation.
- 10 frontend/player tests using the actual TypeScript modules: serialized edits, stale responses, failed saves, first-load errors, first-streamer retry, time boundaries, pause behavior, drift correction, end-of-window and future retarget.
- TypeScript strict check and Vite production build.
- Rust formatting and Clippy, all targets, warnings denied.
- npm audit: zero advisories at verification time.

## Native end-to-end smoke test

The release executable was launched with a temporary local WebView2 debugging port; the production command bridge, DOM controls, and embedded Twitch video were exercised directly.

- Runtime credential authentication and live Helix schedule fetch succeeded.
- Watch opened the bundled wrapper and Twitch video rendered. The initial HTTP-origin failure was reproduced and fixed by serving player windows over Tauri's HTTPS scheme.
- Playback advanced with ready video data. Seeking away from the scheduled live position was corrected within the five-second sync interval.
- A deliberate pause remained paused across a sync interval. Play resumed playback.
- Changing delay into a future replay window persisted and paused the open player. The original delay was restored.
- Invalid streamer input was rejected; adding and removing a streamer persisted through the Rust command bridge.
- The entire private portable folder was moved, launched with an unrelated working directory, and retained its settings and runtime credentials. It was then restored to its delivery location.
- The clean public folder started with an empty list and missing credentials, showed onboarding, and did not inherit the private app's settings. The missing-credential path was exercised.

## Packaging and privacy

Portable executable, NSIS installer and MSI installer built successfully. Source and public binaries were scanned against local credential values; none were present. Release builds remap local user-profile paths to a neutral build path. Personal followed-streamer defaults were removed from the public app. Private credentials and settings are confined to the ignored personal package; public packages use the same credential-free executable.

Installer installation/uninstallation, physical sleep/resume, a screen reader, and every Twitch restriction/login scenario were not tested. Public playback requires the recipient's own valid Twitch credentials and an available VOD. WebView2 Runtime is required. Supported Twitch controls remain available; this app does not lock the timeline. The basic iframe fallback cannot maintain live sync.

Player methods and events were checked against [Twitch's official embed documentation](https://dev.twitch.tv/docs/embed/video-and-clips/).

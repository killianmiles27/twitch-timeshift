# Twitch Timeshift

**Watch Twitch VODs as if they were live in your timezone.**

A streamer goes live at 6 PM in Los Angeles; you are in Berlin and asleep.
Timeshift replays the VOD on a simulated schedule: pick your timezone and a
delay (say +24 h), and the VOD "starts" at the same local time it originally
aired. Open the app mid-window and you join the VOD at exactly the position a
live viewer would be at — in a locked player with no scrubbing and no
spoilers.

Rebuilt from scratch in **Rust + Tauri v2** (the original was an Electron
prototype). Only the idea carried over.

## How it works

1. Add the streamers you follow.
2. For each, the app fetches the latest VOD from the Twitch API and shifts its
   start by your offset: simulated window = `[vodStart + offset, + duration]`.
3. Streamers sort into **On air now** (join mid-VOD at the live position),
   **Starting soon** (countdown to the simulated start), and **Off air**.
4. **Watch** opens the Twitch player locked at the live position — no
   controls, no timeline, just the stream as it "airs".

## Setup

Download the latest release, run the installer, done. The app ships with its
Twitch API registration built in — no accounts, no keys, no config files.

Settings (streamers, timezone, offset) save automatically to
`config.json` in the app config directory. Offsets from +0 h to +168 h;
presets at +0/+9/+12/+24/+33/+48 h.

## Development

Prereqs: Rust (MSVC), Node 20+, WebView2 (Windows).

```powershell
npm install
npx tauri dev        # dev run (reads .twitch-secrets.json from repo root if present)
cargo test --manifest-path src-tauri/Cargo.toml
.\scripts\build-release.ps1   # release installers in src-tauri/target/release/bundle
```

Credentials resolve in order: `TWITCH_CLIENT_ID`/`TWITCH_CLIENT_SECRET` env
vars → `twitch-secrets.json` beside the app or in the config dir →
compile-time embedded (release builds). See `SPEC.md` for the full contract
and `credentials.rs` for the implementation. Secrets are never committed:
`.twitch-secrets.json` is gitignored.

## License

See `LICENSE` in this repository.

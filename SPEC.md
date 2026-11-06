# Twitch Timeshift — Rebuild Spec (v2.0.0)

Clean-room rewrite of the old Electron "Twichtimeshift2" as **Rust + Tauri v2**.
Only the *idea* and the Twitch API credentials carry over. No old code is reused.

## The idea

Watch a Twitch VOD **as if it were live in your timezone**:

1. The app looks up each followed streamer's latest VOD (Twitch Helix API).
2. The VOD's real start instant is shifted by a user-chosen **offset** (hours).
   That gives a simulated live window `[simStart, simEnd]`.
3. Compared to *now*: inside the window = **LIVE** (you "join" at
   `now - simStart` into the VOD), before it = **STARTING SOON**, after it =
   **OFFLINE**.
4. "Watch" opens a locked Twitch player (no controls, no scrubbing) at the
   exact join timestamp — it feels live, spoilers included (or not).

## Timeshift semantics (normative)

- `simStart = vodCreatedAt + offsetHours`, `simEnd = simStart + vodDuration`.
  All comparisons on UTC instants; the chosen IANA timezone affects **display
  only** (frontend formats with `Intl`, `timeZone` = config timezone).
- State: `now < simStart` → `upcoming`; `simStart <= now < simEnd` → `live`;
  `now >= simEnd` → `offline`. No VOD → `novod`. Unknown login → `notfound`.
  API failure for that streamer → `error`.
- `joinSeconds = now - simStart` (live only). `progress = join / duration`,
  clamped to `0..=1`. `startsInSeconds = simStart - now` (upcoming only).

## Ownership split

- **Pi worker (DeepSeek) owns ONLY these Rust files** — create/implement them,
  plus `#[cfg(test)]` unit tests inside them:
  - `src-tauri/src/twitch.rs` — Helix client, token cache, duration parsing,
    player URL builder.
  - `src-tauri/src/timeshift.rs` — pure timeshift math (no I/O).
  - `src-tauri/src/config_store.rs` — config load/save/validate on disk.
  - `src-tauri/src/commands.rs` — the Tauri commands listed below.
- **Claude owns everything else** (frontend, `lib.rs`, `models.rs`,
  `credentials.rs`, `Cargo.toml`, `tauri.conf.json`, build/release). Do not edit
  files outside the Pi-owned list.

## Frozen contract (do not change without flagging it)

JSON is camelCase everywhere.

```ts
AppConfig        { streamers: string[], timezone: string, offsetHours: number, autoRefreshSeconds: number }
VodInfo          { id: string, title: string, url: string, thumbnailUrl: string|null,
                   createdAt: string /* RFC3339 UTC */, durationSeconds: number }
StreamerStatus   { login: string, displayName: string, avatarUrl: string|null,
                   state: "live"|"upcoming"|"offline"|"novod"|"notfound"|"error",
                   vod: VodInfo|null, simStart: string|null, simEnd: string|null,
                   joinSeconds: number|null, startsInSeconds: number|null,
                   progress: number|null, error: string|null }
StatusReport     { generatedAt: string, timezone: string, offsetHours: number, entries: StreamerStatus[] }
CredentialsStatus{ configured: bool, source: string } // never contains values
```

Tauri commands (invoked from JS by these exact names):

- `get_config() -> AppConfig`
- `save_config(config: AppConfig) -> AppConfig` (validated + normalized + persisted)
- `get_statuses() -> StatusReport` (uses saved config; sorted: live, upcoming,
  offline, novod, notfound, error; alphabetical by login within a group)
- `open_player(vodId: string, joinSeconds: number) -> void`
- `get_credentials_status() -> CredentialsStatus`

Errors: return `Err(String)` with a human message. Never include credential
values in errors or logs.

## Credentials (my code — worker just calls it)

`credentials::load(app_config_dir: Option<&Path>) -> Option<Credentials>` with
`client_id` / `client_secret`, resolution order: env `TWITCH_CLIENT_ID` +
`TWITCH_CLIENT_SECRET` → secrets JSON file (`{ "clientId": "...", "clientSecret": "..." }`
in app config dir or next to the exe) → compile-time `option_env!`.
`credentials::status(...)` mirrors this for the UI badge.

## Twitch API notes

- Token: `POST https://id.twitch.tv/oauth2/token` (`client_credentials`),
  cache until expiry (refresh 60 s early), retry once on 401.
- Users: `GET https://api.twitch.tv/helix/users?login=...` batched (≤100).
- VODs: `GET https://api.twitch.tv/helix/videos?user_id=..&type=archive&first=1&sort=time`
  per user, fetched concurrently. Parse durations like `3h25m10s` / `45m` / `90s`.
- Player: `https://player.twitch.tv/?video={id}&parent=localhost&autoplay=true&controls=false&time={h}h{m}m{s}s`
  in a dedicated Tauri `WebviewWindow` (label `player-{vodId}`; re-open refreshes
  position: close + rebuild if it already exists).

## Config defaults

- `streamers`: the user's migrated list (squeex, ludwig, qtcinderella, hasanabi,
  stanz, jaysontheween, willneff, extraemily, cinna, slimeirl, linustech, kai_cenat)
- `timezone`: system timezone if detectable, else `Europe/Berlin`
- `offsetHours`: 24 · `autoRefreshSeconds`: 60 (0 = off)
- Validate: logins `^[a-z0-9_]{3,25}$` (lowercased, deduped), timezone parses as
  `chrono_tz::Tz`, `offsetHours` clamped `0..=168`, `autoRefreshSeconds` `0..=3600`.
  Persist atomically (temp file + rename), pretty JSON, in `app_config_dir`.

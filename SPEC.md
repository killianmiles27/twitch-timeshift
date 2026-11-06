# Twitch Timeshift — Spec (v2.0.x)

Clean-room rewrite of the old Electron "Twichtimeshift2" as **Rust + Tauri v2**.
Only the *idea* carried over from the prototype. No old code is reused.

**Authorship:** built by Muse Wonder (xhigh), using DeepSeek v4.1 Flash 1017
as a worker via Pi. An earlier draft of this file attributed file ownership
to other parties; that was inaccurate and has been removed.

## The idea

Watch a Twitch VOD **on a delayed, simulated-live schedule**:

1. The app looks up each followed streamer's recent broadcasts (Twitch Helix).
2. Each broadcast's real start instant is shifted by a user-chosen fixed
   **delay** (hours). That gives a simulated window `[simStart, simEnd]`.
3. Compared to *now*: inside the window = **ON AIR** (you "join" at
   `now - simStart` into the VOD), before it = **STARTING SOON**, after it =
   **OFF AIR**.
4. "Watch" opens the VOD in a player window at the live position and keeps
   it synchronized (see "Player" below for exactly what is and isn't
   guaranteed).

## Timeshift semantics (normative)

- `simStart = vodCreatedAt + delayHours`, `simEnd = simStart + vodDuration`.
  All comparisons on UTC instants. The delay is a **fixed number of hours**;
  it does not align a broadcast to a local clock time, and across a
  daylight-saving transition the displayed local start time moves by an
  hour. The chosen IANA timezone affects **display formatting only**.
- When several broadcasts exist, the selected one is deterministic
  (`timeshift::select_window`): a currently-live window first (overlaps →
  latest `simStart`, then longer duration, then list order), else the
  nearest upcoming window, else the most recently ended one. History is
  paginated deep enough to cover the configured delay (100/page, ≤10 pages).
- State: `now < simStart` → `upcoming`; `simStart <= now < simEnd` → `live`;
  `now >= simEnd` → `offline`. No VOD → `novod`. Unknown login → `notfound`.
  API failure for that streamer → `error`.
- Classification uses a fresh timestamp taken after all network requests
  complete; that same instant is the report's `generatedAt`. The frontend
  additionally re-derives states from the windows every second, so cards
  move between sections as boundaries pass without waiting for a refresh.
- `joinSeconds = now - simStart` (live only). `progress = join / duration`,
  clamped to `0..=1`. `startsInSeconds = simStart - now` (upcoming only).
- Durations are re-fetched every refresh, so a still-growing in-progress
  broadcast is re-measured (its shifted window grows with it).

## Player

- The player window loads the bundled `player.html` wrapper, which uses
  Twitch's supported interactive embed (`Twitch.Player` JS API) with the
  numeric Helix video ID; if that fails to become ready, it falls back to
  the documented non-interactive iframe (`video=v{id}&parent=…`).
- Guaranteed: the backend computes the authoritative join position at click
  time and rejects opens outside the simulated window; the wrapper seeks to
  the live position on ready, corrects drift beyond 20 s while playing
  (a deliberate pause is respected; pressing play re-joins live), reports
  opened/ready/playing/paused/blocked/ended/failed states to the main
  window, and retargets in place when the delay changes.
- **Not guaranteed, and never claimed:** Twitch keeps its own player
  controls — timeline scrubbing, volume, fullscreen all remain available.
  There is no supported way to remove them, so this app does not pretend
  the player is "locked". Treat Watch as "join at the live position", not
  as access control.

## Contract

JSON is camelCase everywhere.

```ts
AppConfig        { streamers: string[], timezone: string, offsetHours: number, autoRefreshSeconds: number }
ConfigLoad       { config: AppConfig, firstLaunch: boolean, recovered: boolean, warning: string|null }
VodInfo          { id: string, title: string, url: string, thumbnailUrl: string|null,
                   createdAt: string /* RFC3339 UTC */, durationSeconds: number }
StreamerStatus   { login: string, displayName: string, avatarUrl: string|null,
                   state: "live"|"upcoming"|"offline"|"novod"|"notfound"|"error",
                   vod: VodInfo|null, simStart: string|null, simEnd: string|null,
                   joinSeconds: number|null, startsInSeconds: number|null,
                   progress: number|null, error: string|null }
StatusReport     { generatedAt: string, timezone: string, offsetHours: number, entries: StreamerStatus[] }
CredentialsStatus{ configured: boolean, source: string, authenticated: boolean } // never contains values
OpenPlayerResult { vodId: string, joinSeconds: number }
```

Tauri commands (invoked from JS by these exact names):

- `get_config() -> ConfigLoad`
- `save_config(config: AppConfig) -> AppConfig` (validated + normalized + persisted)
- `get_statuses() -> StatusReport` (entries sorted: live, upcoming by
  `simStart`, offline, novod, notfound, error; alphabetical within groups)
- `open_player(vodId: string) -> OpenPlayerResult` (backend-authoritative
  position; errors outside the simulated window)
- `report_player_state(vodId, state, detail?)` (player wrapper → main window)
- `close_player(vodId: string)`
- `get_credentials_status() -> CredentialsStatus` (`configured` = values
  present; `authenticated` only becomes true after Twitch accepts them)

Settings persistence is driven from a single authoritative desired config
in the frontend: mutations apply synchronously, saves serialize/coalesce,
stale save/refresh responses are discarded by revision, and a refresh
requested mid-flight queues one follow-up.

## Credentials

Resolution order (first hit wins), values never logged/returned:

1. Env `TWITCH_CLIENT_ID` + `TWITCH_CLIENT_SECRET`
2. `twitch-secrets.json` (`{ "clientId": "…", "clientSecret": "…" }`) in the
   app config dir, next to the exe, or the working directory (dev)

Twitch requires client secrets to stay confidential, so distributable
release builds embed nothing: users supply their own Twitch app credentials
(registering an app is free; the client ID is public, the secret is theirs).
`scripts/build-release.ps1 -Personal` copies runtime credentials into the private portable folder. No build embeds credentials. The shared API client is rebuilt whenever the full credentials (ID +
secret) change; cached tokens never outlive their true expiry, and a 401
only invalidates the exact token that was rejected.

## Config defaults & recovery

- `streamers`: empty for public installs; personal portable packages carry their list in a private config file.
- `timezone`: system timezone if detectable, else `Europe/Berlin`
- `offsetHours`: 24 · `autoRefreshSeconds`: 60 (0 = off)
- Loading migrates per-field: a missing/older field falls back individually
  and never discards the streamer list. A corrupt file is copied to
  `config.json.bak`, left untouched on disk, and reported in the UI; the
  next successful save replaces it. Saves are atomic (temp + rename).
- Validate: logins `^[a-z0-9_]{3,25}$` (lowercased, deduped), timezone
  parses as `chrono_tz::Tz`, delay clamped `0..=168` (fractions allowed),
  `autoRefreshSeconds` `0..=3600`.

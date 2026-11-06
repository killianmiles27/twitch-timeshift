import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";

/**
 * Player wrapper: uses Twitch's supported interactive embed (Twitch.Player
 * JS API) inside a bundled local page.
 *
 * Honest scope (BUG_REVIEW #4/#8/#9): Twitch keeps its own controls —
 * including timeline, volume, and fullscreen. This wrapper does NOT remove
 * or block them. What it guarantees is narrower: playback starts at the
 * simulated live position, drift past the tolerance is corrected while
 * playing, and playback states (ready / playing / blocked / ended / failed)
 * are reported to the main window instead of pretending window creation
 * means success.
 */

const DRIFT_TOLERANCE_SECONDS = 20;
const SYNC_INTERVAL_MS = 5_000;
const READY_TIMEOUT_MS = 15_000;

const params = new URLSearchParams(location.search);
const vodId = params.get("vod") ?? "";
let simStartMs = Number(params.get("start") ?? "0");
let durationSeconds = Number(params.get("dur") ?? "0");
const fallbackUrl = params.get("fb");

const statusEl = document.getElementById("status") as HTMLElement;
const overlayEl = document.getElementById("overlay") as HTMLElement;
const overlayTitle = document.getElementById("overlayTitle") as HTMLElement;
const overlayText = document.getElementById("overlayText") as HTMLElement;
const playerHost = document.getElementById("player") as HTMLElement;
const playBtn = document.getElementById("playBtn") as HTMLButtonElement;
playBtn.addEventListener("click", () => {
  hideOverlay();
  callPlayer("play");
});

(document.getElementById("twitchLink") as HTMLAnchorElement).href =
  `https://www.twitch.tv/videos/${encodeURIComponent(vodId)}`;
document.getElementById("twitchLink")?.addEventListener("click", (event) => {
  event.preventDefault();
  void invoke("open_on_twitch", {vodId}).catch((error) => {
    showOverlay("Could not open browser", String(error));
    void report("failed", "browser could not open Twitch");
  });
});
document.getElementById("reloadBtn")?.addEventListener("click", () => {
  location.reload();
});

function setStatus(text: string) {
  statusEl.textContent = `Player: ${text}`;
}

async function report(state: string, detail?: string) {
  setStatus(detail ? `${state} — ${detail}` : state);
  try {
    await invoke("report_player_state", {
      vodId,
      state,
      detail: detail ?? null,
    });
  } catch {
    /* main window may be closed; the status bar still informs the user */
  }
}

function showOverlay(title: string, text: string) {
  overlayTitle.textContent = title;
  overlayText.textContent = text;
  overlayEl.hidden = false;
}

function hideOverlay() {
  overlayEl.hidden = true;
  playBtn.hidden = true;
}

const expectedPosition = () =>
  (Date.now() - simStartMs) / 1000;

/* ---------------- fallback: documented non-interactive iframe ---------------- */

let usedFallback = false;
function useFallbackIframe(reason: string) {
  if (usedFallback || !fallbackUrl) return;
  usedFallback = true;
  playerHost.innerHTML = "";
  const iframe = document.createElement("iframe");
  iframe.src = fallbackUrl;
  const source = new URL(iframe.src);
  source.searchParams.set("parent", location.hostname);
  source.searchParams.set("time", `${Math.floor(expectedPosition())}s`);
  iframe.src = source.toString();
  iframe.allow = "autoplay; fullscreen";
  iframe.allowFullscreen = true;
  iframe.title = "Twitch player (basic embed)";
  playerHost.appendChild(iframe);
  void report(
    "fallback",
    `${reason}. Showing the basic Twitch embed; live-position sync is unavailable in this mode.`,
  );
}

/* ---------------- interactive Twitch.Player embed ---------------- */

// The Twitch embed script is loaded from Twitch's CDN; its global is untyped.
// eslint-disable-next-line @typescript-eslint/no-explicit-any
type AnyPlayer = any;

let player: AnyPlayer = null;
let ready = false;
let readyTimer: number | undefined;

function callPlayer(method: string, ...args: unknown[]): unknown {
  try {
    const fn = player?.[method];
    if (typeof fn === "function") return fn.apply(player, args);
  } catch {
    /* unsupported method on this player build: treat as unavailable */
  }
  return undefined;
}

function driftCheck(source: string) {
  if (!ready || usedFallback) return;
  const expected = expectedPosition();
  if (expected < 0) return;
  if (expected >= durationSeconds && durationSeconds > 0) {
    callPlayer("pause");
    showOverlay(
      "Replay window ended",
      "The simulated live window for this VOD has finished. Close this window and refresh the schedule in the main app.",
    );
    void report("ended", "simulated window finished");
    return;
  }
  if (callPlayer("isPaused") === true) return; // respect a deliberate pause
  const current = callPlayer("getCurrentTime");
  if (typeof current !== "number") return;
  if (Math.abs(current - expected) > DRIFT_TOLERANCE_SECONDS) {
    callPlayer("seek", expected);
    void report(
      "resynced",
      `${source}: jumped to the live position (${Math.round(expected)}s)`,
    );
  }
}

function initInteractivePlayer() {
  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  const Twitch = (window as any).Twitch;
  if (!Twitch?.Player) {
    useFallbackIframe("Twitch's interactive player could not load");
    return;
  }

  player = new Twitch.Player("player", {
    width: "100%",
    height: "100%",
    video: vodId,
    parent: [location.hostname],
    autoplay: true,
  });

  const on = (name: string, handler: () => void) => {
    try {
      player.addEventListener(name, handler);
    } catch {
      /* unknown event name on this build: ignore */
    }
  };

  readyTimer = window.setTimeout(() => {
    if (!ready) {
      useFallbackIframe("Twitch's interactive player did not become ready");
    }
  }, READY_TIMEOUT_MS);

  on(Twitch.Player?.READY ?? "ready", () => {
    ready = true;
    window.clearTimeout(readyTimer);
    hideOverlay();
    void report("ready", "player ready");
    const expected = expectedPosition();
    if (expected > 1) callPlayer("seek", expected);
    callPlayer("play");
  });
  on(Twitch.Player?.PLAYING ?? "playing", () => {
    if (expectedPosition() < 0 || expectedPosition() >= durationSeconds) {
      callPlayer("pause");
      showOverlay("Outside the replay window", "Close this player and use Watch during the scheduled replay.");
      return;
    }
    hideOverlay();
    void report("playing", "playback running");
    driftCheck("on play");
  });
  on(Twitch.Player?.PAUSE ?? "pause", () => {
    void report(
      "paused",
      "paused — pressing play returns to the live position",
    );
  });
  on(Twitch.Player?.ENDED ?? "ended", () => {
    showOverlay(
      "Playback ended",
      "Twitch reports the video has ended. If the simulated window is still running, reload the player.",
    );
    void report("ended", "Twitch reported end of video");
  });
  const blocked = () => {
    playBtn.hidden = false;
    showOverlay(
      "Playback blocked",
      "Twitch could not start playback automatically. Press Play below to try again, or open the video on Twitch if it is unavailable or restricted.",
    );
    void report("blocked", "Twitch blocked playback (PLAYBACK_BLOCKED)");
  };
  on(Twitch.Player?.PLAYBACK_BLOCKED ?? "playbackBlocked", blocked);
  on("playbackblocked", blocked);

  window.setInterval(() => driftCheck("sync"), SYNC_INTERVAL_MS);
}

function loadEmbedScript() {
  void report("opened", "player window created, loading Twitch player");
  const script = document.createElement("script");
  script.src = "https://player.twitch.tv/js/embed/v1.js";
  script.async = true;
  script.onload = () => initInteractivePlayer();
  script.onerror = () =>
    useFallbackIframe("Twitch's player script failed to load (network?)");
  document.head.appendChild(script);
}

/* Settings changed in the main window: move this player's window. */
void listen<{ vodId: string; startMs: number; durationSeconds: number }>(
  "player-retarget",
  (event) => {
    if (event.payload.vodId !== vodId) return;
    const changed = simStartMs !== event.payload.startMs;
    simStartMs = event.payload.startMs;
    durationSeconds = event.payload.durationSeconds;
    if (!changed) { driftCheck("duration refresh"); return; }
    if (Date.now() < simStartMs) {
      callPlayer("pause");
      if (usedFallback) playerHost.innerHTML = "";
      showOverlay("Replay has not started", "The new delay moves this replay into the future. Close this player and use Watch when it starts.");
      void report("paused", "new replay window has not started");
      return;
    }
    hideOverlay();
    if (usedFallback) {
      if (expectedPosition() >= durationSeconds) {
        playerHost.innerHTML = "";
        showOverlay("Replay window ended", "Use Watch during an active scheduled replay.");
        return;
      }
      usedFallback = false;
      useFallbackIframe("Schedule changed");
    }
    void report("retargeted", "delay changed; moved to the new live position");
    driftCheck("retarget");
  },
).catch(() => {
  /* event bridge unavailable: initial position still applies */
});

if (!/^\d+$/.test(vodId) || !Number.isFinite(simStartMs) || simStartMs <= 0 || !Number.isFinite(durationSeconds) || durationSeconds <= 0) {
  showOverlay(
    "Missing schedule",
    "This player was opened without a valid simulated window. Close it and use Watch from the main window.",
  );
  void report("failed", "missing or invalid schedule parameters");
} else {
  loadEmbedScript();
}

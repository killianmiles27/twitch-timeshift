import { listen } from "@tauri-apps/api/event";
import {
  getConfig,
  saveConfig,
  getStatuses,
  openPlayer,
  getCredentialsStatus,
  type AppConfig,
  type CredentialsStatus,
  type PlayerStateEvent,
  type StatusReport,
  type StreamerStatus,
} from "./api";

const $ = <T extends HTMLElement>(id: string) =>
  document.getElementById(id) as T;

const els = {
  timezone: $<HTMLSelectElement>("timezoneSelect"),
  presets: $<HTMLDivElement>("offsetPresets"),
  offsetCustom: $<HTMLInputElement>("offsetCustom"),
  offsetError: $<HTMLParagraphElement>("offsetError"),
  refreshBtn: $<HTMLButtonElement>("refreshBtn"),
  manageBtn: $<HTMLButtonElement>("manageBtn"),
  credBadge: $<HTMLSpanElement>("credBadge"),
  lastUpdated: $<HTMLParagraphElement>("lastUpdated"),
  errorBanner: $<HTMLParagraphElement>("errorBanner"),
  playerStatus: $<HTMLParagraphElement>("playerStatus"),
  firstRun: $<HTMLElement>("firstRun"),
  firstRunAdd: $<HTMLButtonElement>("firstRunAdd"),
  sections: $<HTMLElement>("sections"),
  liveList: $<HTMLDivElement>("liveList"),
  soonList: $<HTMLDivElement>("soonList"),
  offList: $<HTMLDivElement>("offList"),
  footTz: $<HTMLSpanElement>("footTz"),
  dialog: $<HTMLDialogElement>("managerDialog"),
  addInput: $<HTMLInputElement>("addInput"),
  addBtn: $<HTMLButtonElement>("addBtn"),
  addError: $<HTMLParagraphElement>("addError"),
  managerList: $<HTMLUListElement>("managerList"),
};

const OFFSET_PRESETS = [0, 9, 12, 24, 33, 48];
const LOGIN_RE = /^[a-zA-Z0-9_]{3,25}$/;
const RESUME_REFRESH_AGE_MS = 30_000;

/* ------------------------------------------------------------------ */
/* Authoritative desired config + serialized persistence (BUG #2/#3)  */
/*                                                                    */
/* `desired` is the single source of truth. Every control mutates it  */
/* synchronously and bumps `revision`; a single save loop persists    */
/* snapshots in order and discards responses that are no longer the   */
/* latest revision, so rapid changes can never undo each other.       */
/* ------------------------------------------------------------------ */

let desired: AppConfig = {
  streamers: [],
  timezone: "Europe/Berlin",
  offsetHours: 24,
  autoRefreshSeconds: 60,
};
let revision = 0;
let savedRevision = 0;
let saveTask: Promise<boolean> | null = null;
let lastSaveError: string | null = null;
let configWarning: string | null = null;

let report: StatusReport | null = null;
let reportStale = false;
let lastFetchAt = 0;
let refreshTask: Promise<void> | null = null;
let refreshWanted = false;
let refreshTimer: number | undefined;

let credStatus: CredentialsStatus | null = null;
let authenticated = false;

const cloneConfig = (c: AppConfig): AppConfig => ({
  ...c,
  streamers: [...c.streamers],
});

function requestSave(): Promise<boolean> {
  if (!saveTask) {
    saveTask = runSaveLoop().finally(() => {
      saveTask = null;
    });
  }
  return saveTask;
}

async function runSaveLoop(): Promise<boolean> {
  while (savedRevision < revision) {
    const snapshot = cloneConfig(desired);
    const snapshotRevision = revision;
    try {
      const stored = await saveConfig(snapshot);
      savedRevision = snapshotRevision;
      lastSaveError = null;
      configWarning = null;
      if (snapshotRevision === revision) {
        // Nothing newer happened while saving: adopt the backend's
        // normalized values. Otherwise the response is stale — discard it
        // and loop to persist the newer desired state.
        desired = cloneConfig(stored);
        syncControls();
        renderManager();
      }
    } catch (err) {
      lastSaveError = String(err);
      showSaveError(lastSaveError);
      return false;
    }
  }
  return true;
}

function showSaveError(message: string) {
  const text = `Settings could not be saved: ${message} Your changes are still shown — press Refresh to retry saving.`;
  if (els.dialog.open) {
    els.addError.textContent = text;
    els.addError.hidden = false;
  }
  showError(text);
}

/** Apply a mutation to the desired config, persist it, optionally refresh. */
async function mutate(
  change: (draft: AppConfig) => void,
  opts: { refetch: boolean },
): Promise<boolean> {
  change(desired);
  revision += 1;
  syncControls();
  renderManager();
  render();
  const ok = await requestSave();
  if (ok && opts.refetch) void requestRefresh();
  if (ok) showError(null);
  return ok;
}

/* ---------------- formatting ---------------- */

const esc = (s: string) =>
  s.replace(/[&<>"']/g, (c) =>
    ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" })[c]!,
  );

function tzFormat(tz: string, opts: Intl.DateTimeFormatOptions, iso: string) {
  return new Intl.DateTimeFormat(undefined, { timeZone: tz, ...opts }).format(
    new Date(iso),
  );
}

const fmtTime = (tz: string, iso: string) =>
  tzFormat(tz, { hour: "2-digit", minute: "2-digit" }, iso);

const fmtDayTime = (tz: string, iso: string) =>
  tzFormat(tz, { weekday: "short", hour: "2-digit", minute: "2-digit" }, iso);

/** Unambiguous window label; includes dates whenever they differ. */
function fmtWindow(tz: string, startIso: string, endIso: string): string {
  const day = (iso: string) =>
    tzFormat(tz, { weekday: "short", day: "numeric", month: "short" }, iso);
  return day(startIso) === day(endIso)
    ? `${fmtDayTime(tz, startIso)} – ${fmtTime(tz, endIso)}`
    : `${fmtDayTime(tz, startIso)} – ${fmtDayTime(tz, endIso)}`;
}

/** 3723 -> "1:02:03"; 65 -> "1:05" */
function fmtElapsed(totalSeconds: number): string {
  const s = Math.max(0, Math.floor(totalSeconds));
  const h = Math.floor(s / 3600);
  const m = Math.floor((s % 3600) / 60);
  const sec = s % 60;
  const mm = h > 0 ? String(m).padStart(2, "0") : String(m);
  return `${h > 0 ? `${h}:` : ""}${mm}:${String(sec).padStart(2, "0")}`;
}

/** 90061 -> "1d 1h"; 3661 -> "1h 1m"; 61 -> "1m"; 5 -> "5s" */
function fmtDuration(totalSeconds: number): string {
  const s = Math.max(0, Math.round(totalSeconds));
  const d = Math.floor(s / 86400);
  const h = Math.floor((s % 86400) / 3600);
  const m = Math.floor((s % 3600) / 60);
  if (d > 0) return `${d}d ${h}h`;
  if (h > 0) return `${h}h ${m}m`;
  if (m > 0) return `${m}m`;
  return `${s}s`;
}

/* ---------------- temporal state (BUG #6) ---------------- */

type TemporalState = "live" | "upcoming" | "offline";

/** The state an entry is in *right now*, derived from its window. */
function deriveState(e: StreamerStatus, nowMs: number): TemporalState | null {
  if (!e.simStart || !e.simEnd) return null;
  const start = Date.parse(e.simStart);
  const end = Date.parse(e.simEnd);
  if (nowMs < start) return "upcoming";
  if (nowMs < end) return "live";
  return "offline";
}

function effectiveState(e: StreamerStatus, nowMs: number) {
  return deriveState(e, nowMs) ?? e.state;
}

/* ---------------- live ticking ---------------- */

interface TickTarget {
  entry: StreamerStatus;
  joinEl?: HTMLElement;
  countdownEl?: HTMLElement;
  progressEl?: HTMLElement;
}
let tickTargets: TickTarget[] = [];

function tick() {
  if (!report) return;
  const now = Date.now();
  // Reclassify first: a card whose window has started/ended moves sections
  // on this tick instead of waiting for the next network refresh.
  let crossed = false;
  for (const e of report.entries) {
    const derived = deriveState(e, now);
    if (
      derived &&
      e.state !== derived &&
      (e.state === "live" || e.state === "upcoming" || e.state === "offline")
    ) {
      e.state = derived;
      crossed = true;
    }
  }
  if (crossed) {
    render();
    return;
  }
  for (const t of tickTargets) {
    const e = t.entry;
    if (!e.simStart || !e.vod) continue;
    if (effectiveState(e, now) === "live") {
      const join = (now - Date.parse(e.simStart)) / 1000;
      if (t.joinEl) {
        t.joinEl.textContent =
          join <= e.vod.durationSeconds ? fmtElapsed(join) : "ended";
      }
      if (t.progressEl) {
        const p = Math.min(1, Math.max(0, join / e.vod.durationSeconds));
        t.progressEl.style.width = `${(p * 100).toFixed(1)}%`;
      }
    } else if (effectiveState(e, now) === "upcoming" && t.countdownEl) {
      const left = (Date.parse(e.simStart) - now) / 1000;
      t.countdownEl.textContent =
        left > 0 ? `starts in ${fmtElapsed(left)}` : "starting…";
    }
  }
}

/* ---------------- rendering ---------------- */

function avatarHtml(e: StreamerStatus): string {
  if (e.avatarUrl) {
    return `<img class="avatar" src="${esc(e.avatarUrl)}" alt="" loading="lazy" data-avatar="${esc(e.login)}" />`;
  }
  const letter = esc((e.displayName || e.login).charAt(0).toUpperCase() || "?");
  return `<span class="avatar avatar-fallback" aria-hidden="true">${letter}</span>`;
}

function wireAvatarFallbacks(root: ParentNode) {
  root.querySelectorAll<HTMLImageElement>("img[data-avatar]").forEach((img) => {
    img.addEventListener("error", () => {
      const span = document.createElement("span");
      span.className = "avatar avatar-fallback";
      span.setAttribute("aria-hidden", "true");
      span.textContent = (img.dataset.avatar ?? "?").charAt(0).toUpperCase();
      img.replaceWith(span);
    });
  });
}

function liveCard(e: StreamerStatus, tz: string): string {
  const vod = e.vod!;
  return `
  <article class="card" data-login="${esc(e.login)}">
    <div class="card-head">
      ${avatarHtml(e)}
      <div class="card-names">
        <div class="card-name">${esc(e.displayName)}</div>
        <div class="card-vod">${esc(vod.title)}</div>
      </div>
      <span class="badge badge-live">Live</span>
    </div>
    <div class="join-block">
      <span class="join-time" data-join>–</span>
      <span class="join-label">into the VOD,<br />at the live position</span>
    </div>
    <div class="progress"><i data-progress style="width:0%"></i></div>
    <p class="window-line">Simulated ${fmtWindow(tz, e.simStart!, e.simEnd!)} · VOD length ${fmtDuration(vod.durationSeconds)}</p>
    <button class="btn btn-primary btn-watch" type="button" data-watch="${esc(vod.id)}">Watch at live position</button>
  </article>`;
}

function soonCard(e: StreamerStatus, tz: string): string {
  const vod = e.vod!;
  return `
  <article class="card" data-login="${esc(e.login)}">
    <div class="card-head">
      ${avatarHtml(e)}
      <div class="card-names">
        <div class="card-name">${esc(e.displayName)}</div>
        <div class="card-vod">${esc(vod.title)}</div>
      </div>
      <span class="badge badge-soon">Soon</span>
    </div>
    <div class="countdown-time" data-countdown>–</div>
    <p class="window-line">Simulated start ${fmtDayTime(tz, e.simStart!)} · runs ${fmtDuration(vod.durationSeconds)}</p>
  </article>`;
}

function offRow(e: StreamerStatus, tz: string): string {
  let badge = '<span class="badge badge-off">Off air</span>';
  let note = e.simEnd ? `window ended ${fmtDayTime(tz, e.simEnd)}` : "no recent VOD";
  let noteCls = "row-note";
  if (e.state === "novod") {
    note = "no VOD found";
  } else if (e.state === "notfound") {
    badge = '<span class="badge badge-err">Not found</span>';
    note = "check the username";
  } else if (e.state === "error") {
    badge = '<span class="badge badge-err">Error</span>';
    note = e.error ?? "request failed";
    noteCls = "row-note err";
  }
  return `
  <div class="row" data-login="${esc(e.login)}">
    ${avatarHtml(e)}
    <span class="row-name">${esc(e.displayName)}</span>
    ${badge}
    <span class="${noteCls}">${esc(note)}</span>
  </div>`;
}

function setEmpty(listEl: HTMLElement, isEmpty: boolean) {
  const note = document.querySelector<HTMLElement>(
    `[data-empty-for="${listEl.id}"]`,
  );
  if (note) note.hidden = !isEmpty;
}

function render() {
  const hasStreamers = desired.streamers.length > 0;
  els.firstRun.hidden = hasStreamers;
  els.sections.hidden = !hasStreamers;
  if (!hasStreamers || !report) {
    tickTargets = [];
    return;
  }

  const tz = desired.timezone;
  const now = Date.now();
  const live = report.entries.filter(
    (e) => desired.streamers.includes(e.login) && effectiveState(e, now) === "live",
  );
  const soon = report.entries.filter(
    (e) => desired.streamers.includes(e.login) && effectiveState(e, now) === "upcoming",
  );
  const off = report.entries.filter((e) => {
    const s = effectiveState(e, now);
    return desired.streamers.includes(e.login) && s !== "live" && s !== "upcoming";
  });

  els.liveList.innerHTML = live.map((e) => liveCard(e, tz)).join("");
  els.soonList.innerHTML = soon.map((e) => soonCard(e, tz)).join("");
  els.offList.innerHTML = off.map((e) => offRow(e, tz)).join("");
  setEmpty(els.liveList, live.length === 0);
  setEmpty(els.soonList, soon.length === 0);
  setEmpty(els.offList, off.length === 0);

  tickTargets = [];
  const collect = (listEl: HTMLElement, entries: StreamerStatus[]) => {
    for (const e of entries) {
      const card = listEl.querySelector<HTMLElement>(
        `[data-login="${CSS.escape(e.login)}"]`,
      );
      if (!card) continue;
      tickTargets.push({
        entry: e,
        joinEl: card.querySelector<HTMLElement>("[data-join]") ?? undefined,
        countdownEl:
          card.querySelector<HTMLElement>("[data-countdown]") ?? undefined,
        progressEl:
          card.querySelector<HTMLElement>("[data-progress]") ?? undefined,
      });
    }
  };
  collect(els.liveList, live);
  collect(els.soonList, soon);
  wireAvatarFallbacks(els.liveList);
  wireAvatarFallbacks(els.soonList);
  wireAvatarFallbacks(els.offList);

  els.liveList
    .querySelectorAll<HTMLButtonElement>("[data-watch]")
    .forEach((btn) => {
      btn.disabled = savedRevision < revision || report!.offsetHours !== desired.offsetHours;
      btn.addEventListener("click", () => void onWatch(btn));
    });

  const updated = new Intl.DateTimeFormat(undefined, {
    hour: "2-digit",
    minute: "2-digit",
    second: "2-digit",
  }).format(new Date(report.generatedAt));
  els.lastUpdated.textContent = reportStale
    ? `Stale — last updated ${updated}`
    : `Updated ${updated}`;
  tick();
}

function renderSkeletons() {
  const sk = `
    <div class="card skeleton" aria-hidden="true">
      <div class="sk sk-line w40"></div>
      <div class="sk sk-line w70"></div>
      <div class="sk sk-line w55"></div>
      <div class="sk sk-line w40"></div>
    </div>`;
  els.liveList.innerHTML = sk.repeat(3);
  els.soonList.innerHTML = "";
  els.offList.innerHTML = "";
}

/** Explicit failure state (BUG #11): never leave skeletons spinning. */
function renderInitialFailure(message: string) {
  els.firstRun.hidden = true;
  els.sections.hidden = false;
  els.liveList.innerHTML = `
    <div class="failed-panel">
      <h3>Could not load the schedule</h3>
      <p>${esc(message)}</p>
      <button class="btn btn-primary" type="button" data-retry>Retry</button>
    </div>`;
  els.soonList.innerHTML = "";
  els.offList.innerHTML = "";
  setEmpty(els.liveList, false);
  setEmpty(els.soonList, true);
  setEmpty(els.offList, true);
  els.liveList
    .querySelector<HTMLButtonElement>("[data-retry]")
    ?.addEventListener("click", () => {
      renderSkeletons();
      void requestRefresh();
    });
}

/* ---------------- data flow (BUG #2) ---------------- */

function showError(message: string | null) {
  els.errorBanner.hidden = message === null;
  els.errorBanner.textContent = message ?? "";
}

function requestRefresh(): Promise<void> {
  if (refreshTask) {
    // A fetch is already running: queue exactly one follow-up so a settings
    // change mid-flight is always honored (BUG_REVIEW #2).
    refreshWanted = true;
    return refreshTask;
  }
  refreshTask = runRefreshLoop().finally(() => {
    refreshTask = null;
  });
  return refreshTask;
}

async function runRefreshLoop(): Promise<void> {
  setFetchingUi(true);
  try {
    do {
      refreshWanted = false;
      // Fetch against what is actually persisted: drain pending saves first.
      if (savedRevision < revision && !(await requestSave())) {
        // Never label an old on-disk schedule as the user's unsaved settings.
        if (!report) renderInitialFailure(lastSaveError ?? "Settings could not be saved.");
        return;
      }
      const fetchRevision = revision;
      await refreshCredBadge();
      try {
        const rep = await getStatuses();
        if (fetchRevision !== revision) {
          // Settings changed while this response was in flight: it is
          // obsolete. Never render it; fetch again for the new revision.
          refreshWanted = true;
          continue;
        }
        report = rep;
        reportStale = false;
        lastFetchAt = Date.now();
        authenticated = desired.streamers.length > 0 && rep.entries.some((e) => e.state !== "error");
        renderCredBadge();
        showError(lastSaveError ? `Settings could not be saved: ${lastSaveError}` : configWarning);
        render();
      } catch (err) {
        if (fetchRevision !== revision) {
          refreshWanted = true;
          continue;
        }
        const message = String(err);
        lastFetchAt = Date.now();
        if (/credential|token/i.test(message)) {
          authenticated = false;
          renderCredBadge();
        }
        if (report && report.entries.length > 0) {
          // Transient failure: keep the useful data, clearly marked stale.
          reportStale = true;
          showError(
            `Refresh failed (${message}). Showing the last known schedule.`,
          );
          render();
        } else {
          showError(`Could not load streamer status: ${message}`);
          renderInitialFailure(message);
        }
      }
    } while (refreshWanted);
  } finally {
    setFetchingUi(false);
    scheduleAutoRefresh();
  }
}

function setFetchingUi(fetching: boolean) {
  els.refreshBtn.disabled = fetching;
  els.refreshBtn.querySelector("svg")?.classList.toggle("spin", fetching);
}

function scheduleAutoRefresh() {
  window.clearTimeout(refreshTimer);
  const secs = desired.autoRefreshSeconds;
  if (secs > 0) {
    refreshTimer = window.setTimeout(() => void requestRefresh(), secs * 1000);
  }
}

async function onWatch(btn: HTMLButtonElement) {
  if (savedRevision < revision || report?.offsetHours !== desired.offsetHours) {
    showError("Wait for the updated schedule before opening a player.");
    return;
  }
  const vodId = btn.dataset.watch;
  if (!vodId || !report) return;
  const entry = report.entries.find((e) => e.vod?.id === vodId);
  if (!entry?.simStart || !entry.simEnd) return;

  // Client-side window check for instant feedback; the backend recomputes
  // the authoritative position and rejects out-of-window opens (BUG #7).
  const now = Date.now();
  if (now < Date.parse(entry.simStart)) {
    showError("This replay has not started yet.");
    return;
  }
  if (now >= Date.parse(entry.simEnd)) {
    showError("This replay window has ended. Refreshing the schedule…");
    tick();
    void requestRefresh();
    return;
  }

  btn.disabled = true;
  showPlayerStatus(`Opening player for ${entry.displayName}…`, false);
  try {
    await openPlayer(vodId);
  } catch (err) {
    showError(`Could not open the player: ${String(err)}`);
    showPlayerStatus(`Player failed to open: ${String(err)}`, true);
  } finally {
    btn.disabled = false;
  }
}

function showPlayerStatus(text: string, isError: boolean) {
  els.playerStatus.textContent = text;
  els.playerStatus.hidden = false;
  els.playerStatus.classList.toggle("failed", isError);
  els.playerStatus.classList.toggle("blocked", /blocked/i.test(text));
}

function wirePlayerEvents() {
  void listen<PlayerStateEvent>("player-state", (event) => {
    const { vodId, state, detail } = event.payload;
    const label =
      report?.entries.find((e) => e.vod?.id === vodId)?.displayName ?? vodId;
    showPlayerStatus(
      `Player (${label}): ${state}${detail ? ` — ${detail}` : ""}`,
      state === "failed" || state === "blocked",
    );
  }).catch(() => {
    /* event bridge unavailable: Watch still reports open failures */
  });
}

/* ---------------- controls ---------------- */

function syncControls() {
  els.timezone.value = desired.timezone;
  els.footTz.textContent = desired.timezone.replace(/_/g, " ");
  if (document.activeElement !== els.offsetCustom) {
    els.offsetCustom.value = String(desired.offsetHours);
  }
  els.presets
    .querySelectorAll<HTMLButtonElement>("button")
    .forEach((b) =>
      b.setAttribute(
        "aria-pressed",
        String(Number(b.dataset.hours) === desired.offsetHours),
      ),
    );
}

function buildControls() {
  const zones = new Set<string>();
  try {
    for (const z of Intl.supportedValuesOf("timeZone")) zones.add(z);
  } catch {
    /* older webview: fall back to a short list below */
  }
  if (zones.size === 0) {
    [
      "Europe/Berlin", "Europe/London", "America/New_York",
      "America/Chicago", "America/Denver", "America/Los_Angeles",
      "Asia/Tokyo", "Australia/Sydney", "UTC",
    ].forEach((z) => zones.add(z));
  }
  zones.add(desired.timezone);
  els.timezone.innerHTML = [...zones]
    .sort()
    .map(
      (z) =>
        `<option value="${esc(z)}">${esc(z.replace(/_/g, " "))}</option>`,
    )
    .join("");

  els.presets.innerHTML = OFFSET_PRESETS.map(
    (h) =>
      `<button type="button" data-hours="${h}" aria-pressed="false">+${h}h</button>`,
  ).join("");
  els.presets
    .querySelectorAll<HTMLButtonElement>("button")
    .forEach((b) =>
      b.addEventListener("click", () => {
        void mutate(
          (d) => {
            d.offsetHours = Number(b.dataset.hours);
          },
          { refetch: true },
        );
      }),
    );
}

function wireControls() {
  els.timezone.addEventListener("change", () => {
    const timezone = els.timezone.value;
    // Display-only change: no refetch needed, entries re-render in the new
    // zone. Still persisted through the same serialized path (BUG #3).
    void mutate(
      (d) => {
        d.timezone = timezone;
      },
      { refetch: false },
    ).then((ok) => {
      if (ok) render();
    });
  });

  els.offsetCustom.addEventListener("change", () => {
    const raw = els.offsetCustom.value.trim();
    if (raw === "") {
      // Clearing the field must not become a 0-hour delay (BUG #17).
      els.offsetError.textContent =
        "Delay kept unchanged. Enter a value between 0 and 168 hours (fractions like 1.5 are allowed).";
      els.offsetError.hidden = false;
      syncControls();
      return;
    }
    const value = Number(raw);
    if (!Number.isFinite(value) || value < 0 || value > 168) {
      els.offsetError.textContent =
        "Delay must be between 0 and 168 hours (fractions like 1.5 are allowed).";
      els.offsetError.hidden = false;
      syncControls();
      return;
    }
    els.offsetError.hidden = true;
    if (value === desired.offsetHours) return;
    void mutate(
      (d) => {
        d.offsetHours = value;
      },
      { refetch: true },
    );
  });

  els.refreshBtn.addEventListener("click", () => void requestRefresh());
  els.firstRunAdd.addEventListener("click", () => {
    renderManager();
    els.dialog.showModal();
  });
}

/* ---------------- streamer manager (BUG #14) ---------------- */

function renderManager() {
  els.managerList.innerHTML = desired.streamers
    .map(
      (login) => `
      <li>
        <span>${esc(login)}</span>
        <button class="icon-btn remove-btn" type="button" data-remove="${esc(login)}" aria-label="Remove ${esc(login)}">
          <svg viewBox="0 0 24 24" aria-hidden="true"><path d="M6 6l12 12M18 6L6 18" stroke="currentColor" stroke-width="2" stroke-linecap="round"/></svg>
        </button>
      </li>`,
    )
    .join("");
  els.managerList
    .querySelectorAll<HTMLButtonElement>("[data-remove]")
    .forEach((b) =>
      b.addEventListener("click", () => {
        const login = b.dataset.remove!;
        void mutate(
          (d) => {
            d.streamers = d.streamers.filter((s) => s !== login);
          },
          { refetch: true },
        );
      }),
    );
}

function wireManager() {
  els.manageBtn.addEventListener("click", () => {
    renderManager();
    if (!lastSaveError) els.addError.hidden = true;
    els.dialog.showModal();
    els.addInput.focus();
  });

  const tryAdd = async () => {
    const login = els.addInput.value.trim().toLowerCase();
    if (!LOGIN_RE.test(login)) {
      els.addError.textContent =
        "Usernames are 3–25 characters: letters, numbers, underscores.";
      els.addError.hidden = false;
      return;
    }
    if (desired.streamers.includes(login)) {
      if (lastSaveError && await requestSave()) {
        if (els.addInput.value.trim().toLowerCase() === login) els.addInput.value = "";
        els.addError.hidden = true;
        void requestRefresh();
        return;
      }
      els.addError.textContent = `${login} is already in your list.`;
      els.addError.hidden = false;
      return;
    }
    const ok = await mutate(
      (d) => {
        d.streamers = [...d.streamers, login].sort();
      },
      { refetch: true },
    );
    if (ok) {
      // Only clear the input once the streamer is actually saved; a failed
      // save leaves the text in place with the error above it (BUG #14).
      if (els.addInput.value.trim().toLowerCase() === login) els.addInput.value = "";
      els.addError.hidden = true;
    }
  };
  els.addBtn.addEventListener("click", () => void tryAdd());
  els.addInput.addEventListener("keydown", (ev) => {
    if (ev.key === "Enter") {
      ev.preventDefault();
      void tryAdd();
    }
  });
}

/* ---------------- credentials badge (BUG #16) ---------------- */

function renderCredBadge() {
  const status = credStatus;
  if (!status) return;
  if (!status.configured) {
    els.credBadge.textContent = "API: not configured";
    els.credBadge.classList.add("missing");
    els.credBadge.classList.remove("ok");
    return;
  }
  els.credBadge.textContent = authenticated
    ? `API: authenticated (${status.source})`
    : `API: configured (${status.source}), not yet verified`;
  els.credBadge.classList.toggle("ok", authenticated);
  els.credBadge.classList.remove("missing");
}

async function refreshCredBadge() {
  try {
    credStatus = await getCredentialsStatus();
    renderCredBadge();
    if (credStatus && !credStatus.configured) {
      showError(
        "Twitch API credentials not found. For this build, supply your own Twitch app credentials: " +
          "set TWITCH_CLIENT_ID and TWITCH_CLIENT_SECRET, or place a twitch-secrets.json " +
          "({\"clientId\": …, \"clientSecret\": …}) next to the app, then Refresh.",
      );
    }
  } catch {
    /* badge stays in its neutral state */
  }
}

/* ---------------- sleep / resume (BUG #6) ---------------- */

let hiddenAt = 0;
function wireResume() {
  document.addEventListener("visibilitychange", () => {
    if (document.hidden) {
      hiddenAt = Date.now();
      return;
    }
    tick();
    // After sleep/backgrounding, wall-clock may have jumped: catch up.
    if (Date.now() - hiddenAt > 5_000 && Date.now() - lastFetchAt > RESUME_REFRESH_AGE_MS) {
      void requestRefresh();
    } else if (hiddenAt > 0 && Date.now() - lastFetchAt > RESUME_REFRESH_AGE_MS) {
      void requestRefresh();
    }
  });
  window.addEventListener("focus", () => {
    tick();
    if (lastFetchAt > 0 && Date.now() - lastFetchAt > RESUME_REFRESH_AGE_MS) {
      void requestRefresh();
    }
  });
}

/* ---------------- boot (BUG #11) ---------------- */

async function boot() {
  renderSkeletons();
  try {
    const load = await getConfig();
    desired = cloneConfig(load.config);
    configWarning = load.warning;
    if (load.warning) showError(load.warning);
  } catch (err) {
    // Controls must stay usable even when settings cannot be loaded.
    showError(
      `Settings could not be loaded (${String(err)}). Defaults are shown; saving will retry.`,
    );
  }
  buildControls();
  syncControls();
  wireControls();
  wireManager();
  wirePlayerEvents();
  wireResume();
  render();
  await refreshCredBadge();
  await requestRefresh();
  window.setInterval(tick, 1000);
}

void boot();

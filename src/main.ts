import {
  getConfig,
  saveConfig,
  getStatuses,
  openPlayer,
  getCredentialsStatus,
  type AppConfig,
  type CredentialsStatus,
  type StatusReport,
  type StreamerStatus,
} from "./api";

const $ = <T extends HTMLElement>(id: string) =>
  document.getElementById(id) as T;

const els = {
  timezone: $<HTMLSelectElement>("timezoneSelect"),
  presets: $<HTMLDivElement>("offsetPresets"),
  offsetCustom: $<HTMLInputElement>("offsetCustom"),
  refreshBtn: $<HTMLButtonElement>("refreshBtn"),
  manageBtn: $<HTMLButtonElement>("manageBtn"),
  credBadge: $<HTMLSpanElement>("credBadge"),
  lastUpdated: $<HTMLParagraphElement>("lastUpdated"),
  errorBanner: $<HTMLParagraphElement>("errorBanner"),
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

let config: AppConfig | null = null;
let report: StatusReport | null = null;
let refreshTimer: number | undefined;
let fetching = false;

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
  tzFormat(
    tz,
    { weekday: "short", hour: "2-digit", minute: "2-digit" },
    iso,
  );

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

/* ---------------- live ticking ---------------- */

interface TickTarget {
  entry: StreamerStatus;
  joinEl?: HTMLElement;
  countdownEl?: HTMLElement;
  progressEl?: HTMLElement;
}
let tickTargets: TickTarget[] = [];

function tick() {
  const now = Date.now();
  for (const t of tickTargets) {
    const e = t.entry;
    if (e.state === "live" && e.simStart && e.vod) {
      const join = (now - Date.parse(e.simStart)) / 1000;
      if (t.joinEl) {
        t.joinEl.textContent =
          join <= e.vod.durationSeconds
            ? fmtElapsed(join)
            : "ended";
      }
      if (t.progressEl) {
        const p = Math.min(1, Math.max(0, join / e.vod.durationSeconds));
        t.progressEl.style.width = `${(p * 100).toFixed(1)}%`;
      }
    } else if (e.state === "upcoming" && e.simStart && t.countdownEl) {
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
      <span class="join-label">into the VOD,<br />as if live</span>
    </div>
    <div class="progress"><i data-progress style="width:0%"></i></div>
    <p class="window-line">Simulated ${fmtTime(tz, e.simStart!)} – ${fmtTime(tz, e.simEnd!)} · VOD length ${fmtDuration(vod.durationSeconds)}</p>
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

function render(rep: StatusReport) {
  report = rep;
  const tz = rep.timezone;
  const live = rep.entries.filter((e) => e.state === "live");
  const soon = rep.entries.filter((e) => e.state === "upcoming");
  const off = rep.entries.filter(
    (e) => e.state !== "live" && e.state !== "upcoming",
  );

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
      btn.addEventListener("click", () => void onWatch(btn));
    });

  els.lastUpdated.textContent = `Updated ${new Intl.DateTimeFormat(undefined, { hour: "2-digit", minute: "2-digit", second: "2-digit" }).format(new Date(rep.generatedAt))}`;
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

/* ---------------- data flow ---------------- */

function showError(message: string | null) {
  els.errorBanner.hidden = message === null;
  els.errorBanner.textContent = message ?? "";
}

async function refresh() {
  if (fetching) return;
  fetching = true;
  els.refreshBtn.disabled = true;
  els.refreshBtn.querySelector("svg")?.classList.add("spin");
  try {
    const rep = await getStatuses();
    showError(null);
    render(rep);
  } catch (err) {
    showError(`Could not load streamer status: ${String(err)}`);
  } finally {
    fetching = false;
    els.refreshBtn.disabled = false;
    els.refreshBtn.querySelector("svg")?.classList.remove("spin");
    scheduleAutoRefresh();
  }
}

function scheduleAutoRefresh() {
  window.clearTimeout(refreshTimer);
  const secs = config?.autoRefreshSeconds ?? 0;
  if (secs > 0) {
    refreshTimer = window.setTimeout(() => void refresh(), secs * 1000);
  }
}

async function onWatch(btn: HTMLButtonElement) {
  const vodId = btn.dataset.watch;
  if (!vodId || !report) return;
  const entry = report.entries.find((e) => e.vod?.id === vodId);
  if (!entry?.simStart) return;
  const join = Math.max(
    0,
    Math.floor((Date.now() - Date.parse(entry.simStart)) / 1000),
  );
  btn.disabled = true;
  try {
    await openPlayer(vodId, join);
  } catch (err) {
    showError(`Could not open the player: ${String(err)}`);
  } finally {
    btn.disabled = false;
  }
}

/* ---------------- config / controls ---------------- */

async function persist(next: AppConfig, refetch: boolean) {
  try {
    config = await saveConfig(next);
    syncControls();
    if (refetch) await refresh();
    else if (report) render({ ...report, timezone: config.timezone });
  } catch (err) {
    showError(`Could not save settings: ${String(err)}`);
  }
}

function syncControls() {
  if (!config) return;
  els.timezone.value = config.timezone;
  els.footTz.textContent = config.timezone.replace(/_/g, " ");
  els.offsetCustom.value = String(config.offsetHours);
  els.presets
    .querySelectorAll<HTMLButtonElement>("button")
    .forEach((b) =>
      b.setAttribute(
        "aria-pressed",
        String(Number(b.dataset.hours) === config!.offsetHours),
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
  if (config && !zones.has(config.timezone)) zones.add(config.timezone);
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
        if (!config) return;
        void persist({ ...config, offsetHours: Number(b.dataset.hours) }, true);
      }),
    );
}

function wireControls() {
  els.timezone.addEventListener("change", () => {
    if (!config) return;
    void persist({ ...config, timezone: els.timezone.value }, false);
  });
  els.offsetCustom.addEventListener("change", () => {
    if (!config) return;
    const v = Number(els.offsetCustom.value);
    if (Number.isFinite(v)) {
      void persist(
        { ...config, offsetHours: Math.min(168, Math.max(0, v)) },
        true,
      );
    }
  });
  els.refreshBtn.addEventListener("click", () => void refresh());
}

/* ---------------- streamer manager ---------------- */

function renderManager() {
  if (!config) return;
  els.managerList.innerHTML = config.streamers
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
        if (!config) return;
        const login = b.dataset.remove!;
        void persist(
          { ...config, streamers: config.streamers.filter((s) => s !== login) },
          true,
        ).then(renderManager);
      }),
    );
}

function wireManager() {
  els.manageBtn.addEventListener("click", () => {
    renderManager();
    els.addError.hidden = true;
    els.dialog.showModal();
    els.addInput.focus();
  });
  const tryAdd = () => {
    if (!config) return;
    const login = els.addInput.value.trim().toLowerCase();
    if (!LOGIN_RE.test(login)) {
      els.addError.textContent =
        "Usernames are 3–25 characters: letters, numbers, underscores.";
      els.addError.hidden = false;
      return;
    }
    if (config.streamers.includes(login)) {
      els.addError.textContent = `${login} is already in your list.`;
      els.addError.hidden = false;
      return;
    }
    els.addError.hidden = true;
    els.addInput.value = "";
    void persist(
      { ...config, streamers: [...config.streamers, login].sort() },
      true,
    ).then(renderManager);
  };
  els.addBtn.addEventListener("click", tryAdd);
  els.addInput.addEventListener("keydown", (ev) => {
    if (ev.key === "Enter") {
      ev.preventDefault();
      tryAdd();
    }
  });
}

/* ---------------- boot ---------------- */

function renderCredBadge(status: CredentialsStatus) {
  if (status.configured) {
    els.credBadge.textContent = `API: ready (${status.source})`;
    els.credBadge.classList.add("ok");
    els.credBadge.classList.remove("missing");
  } else {
    els.credBadge.textContent = "API: not configured";
    els.credBadge.classList.add("missing");
    els.credBadge.classList.remove("ok");
    showError(
      "Twitch API credentials not found. Set TWITCH_CLIENT_ID and TWITCH_CLIENT_SECRET, " +
        "or place a twitch-secrets.json ({\"clientId\": …, \"clientSecret\": …}) next to the app, then Refresh.",
    );
  }
}

async function boot() {
  renderSkeletons();
  try {
    config = await getConfig();
  } catch (err) {
    showError(`Could not load settings: ${String(err)}`);
    return;
  }
  buildControls();
  syncControls();
  wireControls();
  wireManager();
  try {
    renderCredBadge(await getCredentialsStatus());
  } catch {
    /* badge stays in its neutral state */
  }
  await refresh();
  window.setInterval(tick, 1000);
}

void boot();

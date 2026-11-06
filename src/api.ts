import { invoke } from "@tauri-apps/api/core";

/** Mirrors the Rust contract in models.rs (camelCase JSON). */

export interface AppConfig {
  streamers: string[];
  timezone: string;
  offsetHours: number;
  autoRefreshSeconds: number;
}

/** Result of loading persisted settings (BUG_REVIEW #12). */
export interface ConfigLoad {
  config: AppConfig;
  firstLaunch: boolean;
  recovered: boolean;
  warning: string | null;
}

export interface VodInfo {
  id: string;
  title: string;
  url: string;
  thumbnailUrl: string | null;
  createdAt: string;
  durationSeconds: number;
}

export type StreamerState =
  | "live"
  | "upcoming"
  | "offline"
  | "novod"
  | "notfound"
  | "error";

export interface StreamerStatus {
  login: string;
  displayName: string;
  avatarUrl: string | null;
  state: StreamerState;
  vod: VodInfo | null;
  simStart: string | null;
  simEnd: string | null;
  joinSeconds: number | null;
  startsInSeconds: number | null;
  progress: number | null;
  error: string | null;
}

export interface StatusReport {
  generatedAt: string;
  timezone: string;
  offsetHours: number;
  entries: StreamerStatus[];
}

export interface CredentialsStatus {
  configured: boolean;
  /** "environment" | "file" | "embedded" | "missing" — never a value. */
  source: string;
  /** True only after Twitch accepted the credentials this session. */
  authenticated: boolean;
}

export interface OpenPlayerResult {
  vodId: string;
  joinSeconds: number;
}

export type PlayerStateName =
  | "opened"
  | "ready"
  | "playing"
  | "paused"
  | "buffering"
  | "resynced"
  | "retargeted"
  | "blocked"
  | "ended"
  | "failed"
  | "fallback";

export interface PlayerStateEvent {
  vodId: string;
  state: PlayerStateName | string;
  detail: string | null;
}

export const getConfig = () => invoke<ConfigLoad>("get_config");

export const saveConfig = (config: AppConfig) =>
  invoke<AppConfig>("save_config", { config });

export const getStatuses = () => invoke<StatusReport>("get_statuses");

/** Backend computes the authoritative join position; no timestamp is sent. */
export const openPlayer = (vodId: string) =>
  invoke<OpenPlayerResult>("open_player", { vodId });

export const closePlayer = (vodId: string) =>
  invoke<void>("close_player", { vodId });

export const getCredentialsStatus = () =>
  invoke<CredentialsStatus>("get_credentials_status");

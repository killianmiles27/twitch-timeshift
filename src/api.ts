import { invoke } from "@tauri-apps/api/core";

/** Mirrors SPEC.md frozen contract (camelCase JSON). */

export interface AppConfig {
  streamers: string[];
  timezone: string;
  offsetHours: number;
  autoRefreshSeconds: number;
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
}

export const getConfig = () => invoke<AppConfig>("get_config");

export const saveConfig = (config: AppConfig) =>
  invoke<AppConfig>("save_config", { config });

export const getStatuses = () => invoke<StatusReport>("get_statuses");

export const openPlayer = (vodId: string, joinSeconds: number) =>
  invoke<void>("open_player", { vodId, joinSeconds });

export const getCredentialsStatus = () =>
  invoke<CredentialsStatus>("get_credentials_status");

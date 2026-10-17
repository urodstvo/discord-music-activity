// Типы, зеркалящие сериализацию Rust (serde переименовывает в camelCase).

export type YnisonStatus =
  | "disconnected"
  | "connecting"
  | "connected"
  | "reconnecting"
  | "authentication_required"
  | "protocol_error";

export type DiscordStatus = "disconnected" | "connecting" | "connected";

export type PlaybackStatus = "playing" | "paused" | "stopped" | "unknown";

export interface PlaybackState {
  status: PlaybackStatus;
  trackId: string | null;
  title: string | null;
  artists: string[];
  album: string | null;
  durationMs: number | null;
  positionMs: number | null;
  coverUrl: string | null;
  trackUrl: string | null;
  sourceDevice: string | null;
  observedAt: number;
}

export interface ErrorReport {
  code: string;
  message: string;
  atUnixMs: number;
}

export interface AppStateSnapshot {
  ynison: YnisonStatus;
  discord: DiscordStatus;
  authRequired: boolean;
  playback: PlaybackState | null;
  presenceEnabled: boolean;
  lastError: ErrorReport | null;
}

export interface AppSettings {
  schemaVersion: number;
  presenceEnabled: boolean;
  showPausedTracks: boolean;
  showTrackTimestamp: boolean;
  showAlbumArtwork: boolean;
  startOnLaunch: boolean;
  closeToTray: boolean;
  minimizeToTray: boolean;
  theme: "system" | "light" | "dark";
  language: "ru";
  diagnosticsEnabled: boolean;
}

export interface DiagnosticsReport {
  appVersion: string;
  tauriVersion: string;
  logDir: string;
  lastError: ErrorReport | null;
  ynisonStatus: string;
  discordStatus: string;
  settings: AppSettings | null;
}

export interface NotificationPayload {
  level: "info" | "warning" | "error";
  message: string;
}

/** Человекочитаемые подписи состояний (UI — по-русски). */
export const YNISON_LABELS: Record<YnisonStatus, string> = {
  disconnected: "Отключено",
  connecting: "Подключение…",
  connected: "Подключено",
  reconnecting: "Переподключение…",
  authentication_required: "Требуется авторизация",
  protocol_error: "Ошибка протокола",
};

export const DISCORD_LABELS: Record<DiscordStatus, string> = {
  disconnected: "Discord не запущен",
  connecting: "Подключение к Discord…",
  connected: "Discord подключён",
};

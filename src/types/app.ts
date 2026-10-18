// Типы, зеркалящие сериализацию Rust (serde переименовывает в camelCase).

export type YnisonStatus =
  | "disconnected"
  | "connecting"
  | "connected"
  | "reconnecting"
  | "authentication_required"
  | "protocol_error";

export type SourceKind = "ynison" | "media_control";

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
  /** Активный источник playback (см. PLAN п.2). */
  source: SourceKind | null;
}

export type SourceMode = "auto" | "ynison" | "media_control";

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
  /** Режим источника playback (PLAN п.2).
   *  auto — Яндекс + SMTC fallback; ynison — только Yandex; media_control — только SMTC. */
  sourceMode: SourceMode;
  /** Фиксация SMTC на процессе (PLAN п.2.1). Пусто — авто-выбор. */
  smtcSourceApp: string;
}

/** Видимая медиа-сессия Windows (команда list_media_sessions). */
export interface MediaSessionInfo {
  appId: string;
  title: string;
  artists: string[];
  status: PlaybackStatus;
  /** Сырая позиция ОС на момент опроса, мс. */
  positionRawMs: number;
  /** Возраст сырого значения (now − LastUpdatedTime), мс. */
  positionAgeMs: number;
  /** Итоговая позиция с экстраполяцией (та же, что уходит в Presence). */
  positionMs: number | null;
  durationMs: number | null;
  /** ОС считает эту сессию текущей. */
  isCurrent: boolean;
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

/** Человекочитаемые подписи активного источника данных. */
export const SOURCE_LABELS: Record<SourceKind, string> = {
  ynison: "Яндекс Музыка (Ynison)",
  media_control: "Windows Media Control (fallback)",
};

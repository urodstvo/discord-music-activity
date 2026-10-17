// Единое состояние приложения: снапшот из backend + события.
// (ТЗ §10.2: frontend получает snapshot + события, не poll каждую секунду.)

import { reactive } from "vue";
import { invoke } from "@tauri-apps/api/core";
import type {
  AppSettings,
  AppStateSnapshot,
  NotificationPayload,
} from "../types/app";

interface Toast {
  id: number;
  level: "info" | "warning" | "error";
  message: string;
}

const state = reactive({
  snapshot: null as AppStateSnapshot | null,
  settings: null as AppSettings | null,
  toasts: [] as Toast[],
  loaded: false,
});

let nextToastId = 1;

function pushToast(level: Toast["level"], message: string): void {
  const id = nextToastId++;
  state.toasts.push({ id, level, message });
  setTimeout(() => {
    state.toasts = state.toasts.filter((toast) => toast.id !== id);
  }, 5000);
}

const appStore = {
  state,
  get snapshot() {
    return state.snapshot;
  },
  get settings() {
    return state.settings;
  },

  async load(): Promise<void> {
    const [snapshot, settings] = await Promise.all([
      invoke<AppStateSnapshot>("get_app_state"),
      invoke<AppSettings>("get_settings"),
    ]);
    state.snapshot = snapshot;
    state.settings = settings;
    state.loaded = true;
  },

  applySnapshot(snapshot: AppStateSnapshot): void {
    state.snapshot = snapshot;
  },

  applySettings(settings: AppSettings): void {
    state.settings = settings;
  },

  applyNotification(payload: NotificationPayload): void {
    pushToast(payload.level, payload.message);
  },

  pushLocalToast(level: Toast["level"], message: string): void {
    pushToast(level, message);
  },

  async saveSettings(partial: Record<string, unknown>): Promise<void> {
    const settings = await Promise.resolve(
      invoke<AppSettings>("update_settings", { partial }),
    );
    state.settings = settings;
  },

  async setPresenceEnabled(enabled: boolean): Promise<void> {
    const settings = await invoke<AppSettings>("set_presence_enabled", {
      enabled,
    });
    state.settings = settings;
  },

  async validateToken(token: string): Promise<void> {
    await invoke("validate_yandex_token", { token });
  },

  async saveToken(token: string): Promise<void> {
    await invoke("save_yandex_token", { token });
  },

  async removeToken(): Promise<void> {
    await invoke("remove_yandex_token");
  },

  async reconnectYandex(): Promise<void> {
    await invoke("reconnect_yandex");
  },

  async reconnectDiscord(): Promise<void> {
    await invoke("reconnect_discord");
  },

  async getDiagnostics() {
    return invoke<import("../types/app").DiagnosticsReport>("get_diagnostics");
  },

  async openLogDirectory(): Promise<void> {
    await invoke("open_log_directory");
  },

  async quitApp(): Promise<void> {
    await invoke("quit_app");
  },
};

export default appStore;
export type AppStore = typeof appStore;

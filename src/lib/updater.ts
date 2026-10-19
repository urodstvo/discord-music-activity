// Обновление приложения через tauri-plugin-updater (ручной режим:
// проверка и установка только по кнопке в Диагностике).
// Релизы и latest.json лежат в GitHub Releases, подпись — minisign
// (pubkey в tauri.conf.json, приватный ключ — в Secrets CI).

import { check } from "@tauri-apps/plugin-updater";

export type UpdaterStage =
  | "idle"
  | "checking"
  | "available"
  | "downloading"
  | "installing"
  | "ready"
  | "up-to-date"
  | "error";

export interface UpdaterState {
  stage: UpdaterStage;
  /** Версия, найденная на сервере (если есть). */
  latestVersion: string | null;
  /** Заметки релиза (body из latest.json). */
  notes: string | null;
  /** Прогресс загрузки 0..100, null — неизвестен. */
  progress: number | null;
  /** Текст ошибки для показа. */
  error: string | null;
}

export function initialUpdaterState(): UpdaterState {
  return {
    stage: "idle",
    latestVersion: null,
    notes: null,
    progress: null,
    error: null,
  };
}

function toMessage(err: unknown): string {
  if (err instanceof Error) return err.message;
  if (typeof err === "string") return err;
  try {
    return JSON.stringify(err);
  } catch {
    return "Неизвестная ошибка";
  }
}

/** Проверить наличие обновления. Возвращает найденную версию или null. */
export async function checkForUpdates(
  state: UpdaterState,
  options?: { timeout?: number },
): Promise<string | null> {
  state.stage = "checking";
  state.error = null;
  state.progress = null;
  try {
    const update = await check({ timeout: options?.timeout ?? 20000 });
    if (!update) {
      state.stage = "up-to-date";
      state.latestVersion = null;
      return null;
    }
    state.stage = "available";
    state.latestVersion = update.version;
    state.notes = update.body ?? null;
    // Освобождаем ресурс Rust: при установке check() вызовем заново.
    await update.close();
    return update.version;
  } catch (err) {
    state.stage = "error";
    state.error = toMessage(err);
    return null;
  }
}

/**
 * Скачать и установить обновление. На Windows установщик сам закроет
 * приложение и перезапустит его после установки — отдельного relaunch
 * не требуется. Прогресс пишется в state.progress.
 */
export async function downloadAndInstallUpdate(
  state: UpdaterState,
): Promise<void> {
  state.stage = "downloading";
  state.error = null;
  state.progress = 0;
  let total = 0;
  let downloaded = 0;
  try {
    // check() заново: предыдущий Update мы закрыли в checkForUpdates.
    const update = await check({ timeout: 20000 });
    if (!update) {
      state.stage = "up-to-date";
      state.progress = null;
      return;
    }
    state.latestVersion = update.version;
    state.notes = update.body ?? null;
    await update.downloadAndInstall((event) => {
      if (event.event === "Started") {
        total = event.data.contentLength ?? 0;
        downloaded = 0;
        state.stage = "downloading";
        state.progress = total > 0 ? 0 : null;
      } else if (event.event === "Progress") {
        downloaded += event.data.chunkLength;
        state.progress = total > 0 ? Math.min(100, Math.round((downloaded / total) * 100)) : null;
      } else if (event.event === "Finished") {
        state.stage = "installing";
        state.progress = 100;
      }
    });
    // На Windows до сюда обычно не доходим: установщик завершает процесс.
    state.stage = "ready";
  } catch (err) {
    state.stage = "error";
    state.error = toMessage(err);
    state.progress = null;
  }
}

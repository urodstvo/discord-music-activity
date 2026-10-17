// Локальный прогресс трека (ТЗ §12.1).
// Таймер привязан к компоненту и гасится при unmount; позиция считается
// от observedAt без запросов к backend каждые десятки секунд.

import { onUnmounted, ref, type Ref } from "vue";
import type { PlaybackState } from "../types/app";

/**
 * Возвращает вычисленную позицию (мс), идущую локально от последнего
 * события Ynison. Обновление раз в секунду — только для визуального
 * прогресса; presence от этого таймера не зависит.
 */
export function usePlaybackClock(
  playback: () => PlaybackState | null,
): Ref<number | null> {
  const positionMs = ref<number | null>(null);

  const tick = (): void => {
    const current = playback();
    if (
      current === null ||
      current.status !== "playing" ||
      current.positionMs === null
    ) {
      positionMs.value = current?.positionMs ?? null;
      return;
    }
    // Позиция протухает со временем: elapsed с момента наблюдения.
    const elapsed = Math.max(0, Date.now() - current.observedAt);
    const base = current.positionMs + elapsed;
    const max = current.durationMs ?? Number.POSITIVE_INFINITY;
    positionMs.value = Math.min(base, max);
  };

  const interval = window.setInterval(tick, 1000);
  tick();

  onUnmounted(() => {
    window.clearInterval(interval);
  });

  return positionMs;
}

/** Форматирование позиции mm:ss. */
export function formatMs(value: number | null): string {
  if (value === null || value < 0) {
    return "--:--";
  }
  const total = Math.floor(value / 1000);
  const minutes = Math.floor(total / 60);
  const seconds = total % 60;
  return `${minutes}:${seconds.toString().padStart(2, "0")}`;
}

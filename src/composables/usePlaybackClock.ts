// Локальный прогресс трека (ТЗ §12.1).
// Таймер привязан к компоненту и гасится при unmount; позиция считается
// от observedAt без запросов к backend каждые десятки секунд.
//
// Сглаживание: backend шлёт новое событие не каждую секунду, а при смене
// трека/статуса/seek. Пересчёт позиции ОС даёт дрожание ±1–2 c, и слепое
// принятие каждого события сбрасывало бы видимый прогресс (тот же эффект
// пользователь видел и в Discord). Поэтому держим якорь (база + момент):
// пока новое событие того же трека в пределах допуска от экстраполяции —
// якорь не двигаем, прогресс монотонен. Новый якорь — только на смену
// трека/статуса, настоящий seek или первую публикацию.

import { onUnmounted, ref, type Ref } from "vue";
import type { PlaybackState } from "../types/app";

/** Допуск расхождения новой позиции с экстраполяцией: мелкое дрожание
 *  игнорируем, настоящий seek (backend публикует только >5 c) принимаем. */
const SMOOTH_TOLERANCE_MS = 3000;

function identityKey(p: PlaybackState): string {
  return `${p.status}|${p.title ?? ""}|${p.artists.join(",")}|${p.album ?? ""}|${p.durationMs ?? ""}`;
}

/**
 * Возвращает вычисленную позицию (мс), идущую локально от последнего
 * события. Обновление раз в секунду — только для визуального
 * прогресса; presence от этого таймера не зависит.
 */
export function usePlaybackClock(
  playback: () => PlaybackState | null,
): Ref<number | null> {
  const positionMs = ref<number | null>(null);

  let anchorPos: number | null = null;
  let anchorAt = 0;
  let anchorKey: string | null = null;

  const tick = (): void => {
    const current = playback();
    const now = Date.now();
    if (
      current === null ||
      current.status !== "playing" ||
      current.positionMs === null
    ) {
      positionMs.value = current?.positionMs ?? null;
      anchorPos = null;
      anchorKey = null;
      return;
    }
    const key = identityKey(current);
    const max = current.durationMs ?? Number.POSITIVE_INFINITY;
    const incoming =
      current.positionMs + Math.max(0, now - current.observedAt);

    if (
      anchorPos !== null &&
      anchorKey === key &&
      Math.abs(incoming - (anchorPos + Math.max(0, now - anchorAt))) <=
        SMOOTH_TOLERANCE_MS
    ) {
      // Дрожание или повтор того же трека — держим старый якорь.
      positionMs.value = Math.min(
        anchorPos + Math.max(0, now - anchorAt),
        max,
      );
      return;
    }
    // Смена трека/статуса, настоящий seek или первая публикация.
    anchorPos = current.positionMs;
    anchorAt = current.observedAt;
    anchorKey = key;
    positionMs.value = Math.min(incoming, max);
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

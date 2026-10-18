<script setup lang="ts">
import { computed } from "vue";
import type { PlaybackState, PlaybackStatus } from "../types/app";

const props = defineProps<{
  playback: PlaybackState | null;
  /** Вычисленная локально позиция (мс) для прогресса в UI. */
  positionMs: number | null;
}>();

const artists = computed(() => props.playback?.artists.join(", ") ?? "");
const hasTrack = computed(
  () =>
    props.playback !== null &&
    (props.playback.trackId !== null || props.playback.title !== null),
);
const coverUrl = computed(() => props.playback?.coverUrl ?? null);
const duration = computed(() => props.playback?.durationMs ?? null);

// Полоса прогресса: нужна длительность и позиция; замораживается на паузе.
const barPercent = computed(() => {
  const dur = props.playback?.durationMs ?? null;
  if (dur === null || dur <= 0 || props.positionMs === null) return null;
  return Math.max(0, Math.min(100, (props.positionMs / dur) * 100));
});

const STATUS_LABELS: Record<PlaybackStatus, string> = {
  playing: "Воспроизводится",
  paused: "Пауза",
  stopped: "Остановлено",
  unknown: "Статус неизвестен",
};

function statusLabel(status: PlaybackStatus): string {
  return STATUS_LABELS[status] ?? status;
}

function humanDuration(ms: number | null): string {
  if (ms === null || ms <= 0) return "--:--";
  const total = Math.floor(ms / 1000);
  const minutes = Math.floor(total / 60);
  const seconds = total % 60;
  return `${minutes}:${String(seconds).padStart(2, "0")}`;
}
</script>

<template>
  <div class="card track-card" data-testid="now-playing">
    <h2 class="section-title">Сейчас играет</h2>

    <template v-if="hasTrack && playback">
      <div class="track-row">
        <img
          v-if="coverUrl"
          :src="coverUrl"
          alt="Обложка альбома"
          class="cover"
        />
        <div v-else class="cover fallback" aria-label="Обложка недоступна">♪</div>

        <div class="track-info">
          <p class="title">{{ playback.title ?? "Неизвестный трек" }}</p>
          <p v-if="artists" class="hint">{{ artists }}</p>
          <p v-if="playback.album" class="hint">{{ playback.album }}</p>
          <p v-if="playback.sourceDevice" class="hint">
            Устройство: {{ playback.sourceDevice }}
          </p>
          <p class="hint status">
            {{ statusLabel(playback.status) }}
            <span
              v-if="
                (playback.status === 'playing' ||
                  playback.status === 'paused') &&
                positionMs !== null
              "
              class="position"
            >
              · {{ humanDuration(positionMs) }} /
              {{ humanDuration(duration) }}
            </span>
          </p>
          <div v-if="barPercent !== null" class="bar-wrap">
            <div class="bar" :style="{ width: barPercent + '%' }"></div>
          </div>
        </div>
      </div>
    </template>

    <div v-else class="empty">
      <div class="empty-icon" aria-hidden="true">🎧</div>
      <p class="empty-title">Ничего не воспроизводится</p>
      <p class="hint">
        Здесь появится трек, как только начнётся воспроизведение в выбранном источнике.
      </p>
    </div>
  </div>
</template>

<style scoped>
.track-card .track-row {
  display: flex;
  gap: 14px;
  align-items: flex-start;
}

.track-info {
  min-width: 0;
}

.title {
  font-weight: 700;
  font-size: 15px;
  margin-bottom: 4px;
  overflow: hidden;
  text-overflow: ellipsis;
  display: -webkit-box;
  -webkit-line-clamp: 2;
  -webkit-box-orient: vertical;
}

.status {
  margin-top: 6px;
  color: var(--text);
}

.position {
  color: var(--text-muted);
}

.bar-wrap {
  margin-top: 8px;
  height: 4px;
  border-radius: 2px;
  background: color-mix(in srgb, var(--text) 15%, transparent);
  overflow: hidden;
}

.bar {
  height: 100%;
  border-radius: 2px;
  background: var(--accent, #fdb30f);
  transition: width 1s linear;
}

.empty {
  text-align: center;
  padding: 24px 0;
}

.empty-icon {
  font-size: 40px;
  margin-bottom: 6px;
}

.empty-title {
  font-weight: 700;
  margin-bottom: 4px;
}
</style>

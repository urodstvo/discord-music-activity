<script setup lang="ts">
import { computed, watch } from "vue";
import appStore from "../stores/appStore";
import { usePlaybackClock } from "../composables/usePlaybackClock";
import ConnectionStatus from "../components/ConnectionStatus.vue";
import NowPlayingCard from "../components/NowPlayingCard.vue";

const snapshot = computed(() => appStore.state.snapshot);
const playback = computed(() => snapshot.value?.playback ?? null);
const positionMs = usePlaybackClock(() => playback.value);

const lastErrorCode = computed(() => snapshot.value?.lastError?.code ?? null);

// Первичная загрузка, если App.vue прошёл раньше, чем снапшот пришёл.
watch(
  () => appStore.state.loaded,
  () => {
    if (!snapshot.value) {
      void appStore.load();
    }
  },
);
</script>

<template>
  <div class="layout-two">
    <div class="col">
      <NowPlayingCard :playback="playback" :position-ms="positionMs" />
    </div>

    <div class="col">
      <div class="card">
        <h2 class="section-title">Соединения</h2>
        <ConnectionStatus />
        <p
          v-if="lastErrorCode"
          class="hint error-note"
          data-testid="last-error"
        >
          Последняя ошибка: {{ snapshot?.lastError?.code }}
          — {{ snapshot?.lastError?.message }}
        </p>
      </div>
    </div>
  </div>
</template>

<style scoped>
.col {
  display: flex;
  flex-direction: column;
  gap: 16px;
  min-width: 0;
}

.error-note {
  margin-top: 10px;
  color: var(--danger);
}
</style>

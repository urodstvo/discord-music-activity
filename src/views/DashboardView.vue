<script setup lang="ts">
import { computed, watch } from "vue";
import appStore from "../stores/appStore";
import { usePlaybackClock } from "../composables/usePlaybackClock";
import ConnectionStatus from "../components/ConnectionStatus.vue";
import NowPlayingCard from "../components/NowPlayingCard.vue";

const snapshot = computed(() => appStore.state.snapshot);
const playback = computed(() => snapshot.value?.playback ?? null);
const positionMs = usePlaybackClock(() => playback.value);

const lastError = computed(() => snapshot.value?.lastError ?? null);
const presenceEnabled = computed(
  () => snapshot.value?.presenceEnabled ?? true,
);
const needsAuth = computed(() => snapshot.value?.authRequired ?? false);

async function togglePresence(): Promise<void> {
  try {
    await appStore.setPresenceEnabled(!presenceEnabled.value);
  } catch {
    appStore.pushLocalToast("error", "Не удалось переключить публикацию");
  }
}

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
        <h2 class="section-title">Статус</h2>
        <div class="row presence-row">
          <div>
            <p title="Показывать текущий трек в профиле Discord">
              Публикация в Discord
            </p>
            <p class="hint">
              {{ presenceEnabled ? "Трек виден в профиле" : "Скрыто от других" }}
            </p>
          </div>
          <label
            class="switch"
            title="Включить или выключить показ музыки в Discord"
          >
            <input
              type="checkbox"
              :checked="presenceEnabled"
              aria-label="Публикация в Discord"
              @change="togglePresence"
            />
            <span class="slider" aria-hidden="true"></span>
          </label>
        </div>
        <ConnectionStatus />
        <p
          v-if="lastError"
          class="hint error-note"
          data-testid="last-error"
          :title="`Код ошибки: ${lastError.code}`"
        >
          {{ lastError.message }}
        </p>
        <p
          v-if="needsAuth"
          class="hint auth-note"
          title="Без токена приложение может брать музыку только из других приложений Windows"
        >
          Нет токена Яндекс Музыки — откройте «Настройки», раздел «Аккаунт».
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

.presence-row {
  border-bottom: 1px solid var(--border);
  margin-bottom: 10px;
  padding-bottom: 12px;
}

.error-note {
  margin-top: 10px;
  color: var(--danger);
}

.auth-note {
  margin-top: 8px;
  color: var(--warning);
}

/* Тумблер — как в настройках, чтобы главная выглядела единообразно. */
.switch {
  position: relative;
  display: inline-block;
  width: 44px;
  height: 24px;
  flex: none;
}

.switch input {
  opacity: 0;
  width: 0;
  height: 0;
}

.slider {
  position: absolute;
  inset: 0;
  background: var(--bg-input);
  border: 1px solid var(--border);
  border-radius: 999px;
  cursor: pointer;
  transition: background 0.15s ease;
}

.slider::before {
  content: "";
  position: absolute;
  width: 18px;
  height: 18px;
  left: 2px;
  top: 50%;
  transform: translateY(-50%);
  border-radius: 50%;
  background: var(--text-muted);
  transition: background 0.15s ease, transform 0.15s ease;
}

.switch input:checked + .slider {
  background: var(--accent);
}

.switch input:checked + .slider::before {
  background: var(--accent-contrast);
  transform: translateY(-50%) translateX(20px);
}

.switch input:focus-visible + .slider {
  outline: 2px solid var(--accent);
}
</style>

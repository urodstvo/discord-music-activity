<script setup lang="ts">
import { onMounted, ref, watch } from "vue";
import appStore from "../stores/appStore";
import { useTauriEvent } from "../composables/useTauriEvents";
import ToastHost from "../components/ToastHost.vue";
import DashboardView from "../views/DashboardView.vue";
import SettingsView from "../views/SettingsView.vue";
import DiagnosticsView from "../views/DiagnosticsView.vue";
import type {
  AppSettings,
  AppStateSnapshot,
  NotificationPayload,
} from "../types/app";

const currentTab = ref<"dashboard" | "settings" | "diagnostics">("dashboard");

onMounted(async () => {
  try {
    await appStore.load();
  } catch (err) {
    appStore.pushLocalToast("error", "Не удалось получить состояние приложения");
    console.warn("load failed", err);
  }
  applyTheme(appStore.settings?.theme ?? "system");
});

useTauriEvent<AppStateSnapshot>("app-state-changed", (snapshot) => {
  appStore.applySnapshot(snapshot);
});
// playback-changed не подписываем отдельно: снапшот (app-state-changed)
// уже содержит актуальный playback, а прогресс считает usePlaybackClock.
useTauriEvent<AppSettings>("settings-changed", (settings) => {
  appStore.applySettings(settings);
  applyTheme(settings.theme);
});
useTauriEvent<NotificationPayload>("notification", (payload) => {
  appStore.applyNotification(payload);
});

function applyTheme(theme: AppSettings["theme"] | undefined): void {
  if (!theme) return;
  const prefersDark = window.matchMedia("(prefers-color-scheme: dark)").matches;
  const resolved =
    theme === "system" ? (prefersDark ? "dark" : "light") : theme;
  document.documentElement.dataset.theme = resolved;
}

watch(
  () => appStore.settings?.theme,
  (theme) => applyTheme(theme),
);
</script>

<template>
  <header class="app-header">
    <nav class="tabs" aria-label="Разделы">
      <button
        :class="{ active: currentTab === 'dashboard' }"
        @click="currentTab = 'dashboard'"
      >
        Главная
      </button>
      <button
        :class="{ active: currentTab === 'settings' }"
        @click="currentTab = 'settings'"
      >
        Настройки
      </button>
      <button
        :class="{ active: currentTab === 'diagnostics' }"
        @click="currentTab = 'diagnostics'"
      >
        Диагностика
      </button>
    </nav>
  </header>

  <main v-if="appStore.state.loaded" class="view">
    <DashboardView v-if="currentTab === 'dashboard'" />
    <SettingsView v-else-if="currentTab === 'settings'" />
    <DiagnosticsView v-else />
  </main>
  <main v-else class="view">
    <div class="card hint">Загрузка…</div>
  </main>

  <ToastHost />
</template>

<style scoped>
.app-header {
  display: flex;
  align-items: center;
  justify-content: flex-start;
  gap: 16px;
  flex-wrap: wrap;
  min-width: 0;
}
</style>

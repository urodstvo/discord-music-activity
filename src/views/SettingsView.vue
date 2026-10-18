<script setup lang="ts">
import { computed, onMounted, ref } from "vue";
import appStore from "../stores/appStore";
import type { AppSettings, MediaSessionInfo } from "../types/app";
import AuthCard from "../components/AuthCard.vue";

const settings = computed(() => appStore.state.settings);
const snapshot = computed(() => appStore.state.snapshot);
const busy = ref(false);

const presenceEnabled = computed(
  () => appStore.state.snapshot?.presenceEnabled ?? true,
);

interface ToggleRow {
  key: keyof AppSettings;
  title: string;
  hint: string;
  testid: string;
}

// Ключи уже camelCase — совпадают с сериализацией Rust (serde camelCase).
const TOGGLES: ToggleRow[] = [
  {
    key: "showPausedTracks",
    title: "Показывать трек на паузе",
    hint: "Presence остаётся, когда воспроизведение поставлено на паузу.",
    testid: "toggle-show-paused",
  },
  {
    key: "showTrackTimestamp",
    title: "Прогресс трека",
    hint: "Отображать таймер прослушивания в Discord.",
    testid: "toggle-timestamp",
  },
  {
    key: "showAlbumArtwork",
    title: "Обложка альбома",
    hint: "Загружать большую картинку из CDN Яндекс Музыки.",
    testid: "toggle-artwork",
  },
  {
    key: "startOnLaunch",
    title: "Запускать при входе в Windows",
    hint: "Приложение стартует вместе с системой.",
    testid: "toggle-autostart",
  },
  {
    key: "closeToTray",
    title: "Закрывать в трей",
    hint: "Кнопка закрытия сворачивает окно в системный трей.",
    testid: "toggle-close-to-tray",
  },
  {
    key: "minimizeToTray",
    title: "Сворачивать в трей",
    hint: "Кнопка сворачивания прячет окно, не завершая приложение.",
    testid: "toggle-minimize-to-tray",
  },
  {
    key: "diagnosticsEnabled",
    title: "Расширенная диагностика",
    hint: "Подробные технические логи (debug). Применяется сразу. Секреты не логируются никогда.",
    testid: "toggle-diagnostics",
  },
];

function isEnabled(key: keyof AppSettings): boolean {
  const value = settings.value?.[key];
  return typeof value === "boolean" ? value : false;
}

function themeValue(): AppSettings["theme"] {
  return settings.value?.theme ?? "system";
}

async function setToggle(key: keyof AppSettings, value: boolean): Promise<void> {
  busy.value = true;
  try {
    await appStore.saveSettings({ [key]: value });
  } catch {
    appStore.pushLocalToast("error", "Не удалось сохранить настройку");
  } finally {
    busy.value = false;
  }
}

async function setTheme(value: AppSettings["theme"]): Promise<void> {
  busy.value = true;
  try {
    await appStore.saveSettings({ theme: value });
  } catch {
    appStore.pushLocalToast("error", "Не удалось сохранить тему");
  } finally {
    busy.value = false;
  }
}

function sourceModeValue(): AppSettings["sourceMode"] {
  return settings.value?.sourceMode ?? "auto";
}

async function setSourceMode(
  value: AppSettings["sourceMode"],
): Promise<void> {
  busy.value = true;
  try {
    await appStore.saveSettings({ sourceMode: value });
  } catch {
    appStore.pushLocalToast("error", "Не удалось сохранить режим источника");
  } finally {
    busy.value = false;
  }
}

// Фиксация SMTC-процесса (PLAN п.2.1): список видимых сейчас медиа-сессий
// опрашивается разово, выбор сохраняется в smtcSourceApp ("": авто).
const mediaSessions = ref<MediaSessionInfo[]>([]);
const sessionsLoading = ref(false);

async function refreshSessions(): Promise<void> {
  sessionsLoading.value = true;
  try {
    mediaSessions.value = await appStore.listMediaSessions();
  } catch {
    appStore.pushLocalToast("error", "Не удалось опросить медиа-сессии");
  } finally {
    sessionsLoading.value = false;
  }
}

function smtcSourceAppValue(): string {
  return settings.value?.smtcSourceApp ?? "";
}

async function setSmtcSourceApp(value: string): Promise<void> {
  busy.value = true;
  try {
    await appStore.saveSettings({ smtcSourceApp: value });
  } catch {
    appStore.pushLocalToast("error", "Не удалось зафиксировать процесс");
  } finally {
    busy.value = false;
  }
}

onMounted(() => {
  void refreshSessions();
});

const SMTC_STATUS_RU: Record<string, string> = {
  playing: "играет",
  paused: "пауза",
  stopped: "стоп",
  unknown: "неизвестно",
};

function fmtMs(value: number | null): string {
  if (value === null || value < 0 || !Number.isFinite(value)) return "--:--";
  const total = Math.floor(value / 1000);
  return `${Math.floor(total / 60)}:${String(total % 60).padStart(2, "0")}`;
}

/** Строка диагностики одной сессии: сырые числа ОС рядом с итогом. */
function sessionDebugLine(session: MediaSessionInfo): string {
  const status = SMTC_STATUS_RU[session.status] ?? session.status;
  const ageSec = (session.positionAgeMs / 1000).toFixed(1);
  return `[${status}] raw ${fmtMs(session.positionRawMs)} + возраст ${ageSec} c → ${fmtMs(session.positionMs)} / ${fmtMs(session.durationMs)}`;
}

async function togglePresence(): Promise<void> {
  try {
    await appStore.setPresenceEnabled(!presenceEnabled.value);
  } catch {
    appStore.pushLocalToast("error", "Не удалось переключить публикацию");
  }
}

async function reconnectDiscord(): Promise<void> {
  try {
    await appStore.reconnectDiscord();
    appStore.pushLocalToast("info", "Ищу Discord…");
  } catch {
    appStore.pushLocalToast("error", "Не удалось переподключить Discord");
  }
}
</script>

<template>
  <div class="view settings-view">
    <div class="col">
      <div class="card toggle-card">
        <h2 class="section-title">Поведение</h2>
        <div v-for="toggle in TOGGLES" :key="toggle.key" class="row">
          <div>
            <p>{{ toggle.title }}</p>
            <p class="hint">{{ toggle.hint }}</p>
          </div>
          <label class="switch" :data-testid="toggle.testid">
            <input
              type="checkbox"
              :checked="isEnabled(toggle.key)"
              :disabled="busy"
              @change="setToggle(toggle.key, ($event.target as HTMLInputElement).checked)"
            />
            <span class="slider" aria-hidden="true"></span>
          </label>
        </div>
      </div>
    </div>

    <div class="col">
      <div class="card">
        <h2 class="section-title">Интерфейс</h2>
        <div class="row">
          <div>
            <p>Тема</p>
            <p class="hint">Светлая, тёмная или системная.</p>
          </div>
          <select
            class="theme-select"
            :value="themeValue()"
            :disabled="busy"
            data-testid="theme-select"
            @change="setTheme(($event.target as HTMLSelectElement).value as AppSettings['theme'])"
          >
            <option value="system">Системная</option>
            <option value="light">Светлая</option>
            <option value="dark">Тёмная</option>
          </select>
        </div>
        <p class="hint footer">
          Настройки хранятся локально и не содержат токена.
        </p>
      </div>

      <div class="card">
        <h2 class="section-title">Источник данных</h2>
        <div class="row">
          <div>
            <p>Откуда брать «сейчас играет»</p>
            <p class="hint">
              Авто: аккаунт Яндекс Музыки, а при отсутствии токена или
              проблемах сети — локальные медиа-сессии Windows.
            </p>
          </div>
          <select
            class="theme-select"
            :value="sourceModeValue()"
            :disabled="busy"
            data-testid="source-mode-select"
            @change="setSourceMode(($event.target as HTMLSelectElement).value as AppSettings['sourceMode'])"
          >
            <option value="auto">Авто</option>
            <option value="ynison">Только Яндекс Музыка</option>
            <option value="media_control">Windows Media Control</option>
          </select>
        </div>
        <p class="hint footer">
          Windows Media Control работает без токена: присенс берётся из
          любых медиа-приложений Windows (без кнопки и обложки).
        </p>
      </div>

      <div class="card">
        <h2 class="section-title">Фиксация процесса</h2>
        <div class="row">
          <div>
            <p>Слушать только это приложение</p>
            <p class="hint">
              Авто держит текущий источник и предпочитает приложения
              браузерам, но при скачках выберите процесс вручную —
              остальные игнорируются. Применяется сразу, без перезапуска.
            </p>
          </div>
          <select
            class="theme-select"
            :value="smtcSourceAppValue()"
            :disabled="busy"
            data-testid="smtc-source-select"
            @change="setSmtcSourceApp(($event.target as HTMLSelectElement).value)"
          >
            <option value="">Авто</option>
            <option
              v-for="session in mediaSessions"
              :key="session.appId"
              :value="session.appId"
            >
              {{ session.appId }} — {{ session.title }}
            </option>
          </select>
        </div>
        <div class="row">
          <div>
            <p class="hint">
              {{
                mediaSessions.length
                  ? `Сейчас видно сессий: ${mediaSessions.length}. Сверьте числа с реальным плеером: позиция ОС + возраст должны давать текущее место трека.`
                  : "Сейчас медиа-сессий не видно — начните воспроизведение и обновите список."
              }}
            </p>
          </div>
          <button
            :disabled="sessionsLoading"
            data-testid="smtc-refresh"
            @click="refreshSessions"
          >
            {{ sessionsLoading ? "Опрос…" : "Обновить список" }}
          </button>
        </div>
        <ul v-if="mediaSessions.length" class="session-list">
          <li
            v-for="session in mediaSessions"
            :key="session.appId + session.title"
            class="session-row"
          >
            <strong>{{ session.appId }}</strong>
            <span v-if="session.isCurrent" class="hint">· текущая по ОС</span>
            <span class="hint">{{ session.title }}</span>
            <span class="hint mono">{{ sessionDebugLine(session) }}</span>
          </li>
        </ul>
      </div>

      <div class="card" data-testid="presence-control">
        <h2 class="section-title">Публикация в Discord</h2>
        <div class="row">
          <div>
            <p>Rich Presence</p>
            <p class="hint">
              {{ presenceEnabled
                ? "Активность публикуется, когда играет музыка."
                : "Публикация выключена." }}
            </p>
          </div>
          <button
            class="primary"
            data-testid="presence-toggle"
            @click="togglePresence"
          >
            {{ presenceEnabled ? "Выключить" : "Включить" }}
          </button>
        </div>
        <div class="row">
          <div>
            <p>Discord</p>
            <p class="hint">Перезапуск подключения, если Discord запустился позже.</p>
          </div>
          <button @click="reconnectDiscord">Переподключить</button>
        </div>
      </div>

      <AuthCard v-if="snapshot" />
    </div>
  </div>
</template>

<style scoped>
/* На малом окне карточки идут одной колонкой на всю ширину (до правого
   края, как на остальных экранах); на большом — две равные колонки:
   слева «Поведение», справа «Интерфейс» + «Публикация» + аккаунт. */
.settings-view {
  display: grid;
  grid-template-columns: repeat(2, minmax(0, 1fr));
  gap: 16px;
  align-items: start;
}

.col {
  display: flex;
  flex-direction: column;
  gap: 16px;
  min-width: 0;
}

/* Та же контрольная точка, что и у главной (layout-two). */
@media (max-width: 780px) {
  .settings-view {
    grid-template-columns: 1fr;
  }
}

.toggle-card .row {
  align-items: center;
}

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

.theme-select {
  width: 160px;
}

.session-list {
  list-style: none;
  margin: 4px 0 0;
  padding: 0;
  display: flex;
  flex-direction: column;
  gap: 8px;
}

.session-row {
  display: flex;
  flex-direction: column;
  gap: 2px;
  padding: 8px 10px;
  border: 1px solid var(--border);
  border-radius: 8px;
}

.mono {
  font-variant-numeric: tabular-nums;
}

.footer {
  margin-top: 10px;
}
</style>

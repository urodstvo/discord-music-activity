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
  /** Всплывающая подсказка с деталями — для тех, кому нужно больше. */
  tip?: string;
  testid: string;
}

// Ключи уже camelCase — совпадают с сериализацией Rust (serde camelCase).
const PRESENCE_TOGGLES: ToggleRow[] = [
  {
    key: "showPausedTracks",
    title: "Трек на паузе",
    hint: "Оставлять трек в статусе при паузе, без бегущего таймера.",
    testid: "toggle-show-paused",
  },
  {
    key: "showTrackTimestamp",
    title: "Таймер трека",
    hint: "Показывать, сколько времени идёт трек.",
    tip: "На паузе таймер убирается, чтобы не вводить в заблуждение.",
    testid: "toggle-timestamp",
  },
  {
    key: "showAlbumArtwork",
    title: "Обложка",
    hint: "Показывать обложку, если Discord её поддерживает.",
    testid: "toggle-artwork",
  },
  {
    key: "showButton",
    title: "Кнопка-ссылка",
    hint: "Честная ссылка на трек под активностью (Яндекс / iTunes / Deezer); скрывается, если трек не опознан.",
    testid: "toggle-button",
  },
];

const APP_TOGGLES: ToggleRow[] = [
  {
    key: "startOnLaunch",
    title: "Запуск вместе с Windows",
    hint: "Приложение стартует свёрнутым в трей.",
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
];

const DIAG_TOGGLE: ToggleRow = {
  key: "diagnosticsEnabled",
  title: "Подробные логи",
  hint: "Нужно только для поиска проблем.",
  tip: "Токен и секреты в логи не пишутся никогда.",
  testid: "toggle-diagnostics",
};

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
</script>

<template>
  <div class="view settings-view">
    <div class="col">
      <div class="card" data-testid="presence-control">
        <h2 class="section-title">Discord</h2>
        <div class="row">
          <div>
            <p title="Показывать текущий трек в профиле Discord">
              Показывать музыку в статусе
            </p>
            <p class="hint">
              {{ presenceEnabled ? "Друзья видят, что играет." : "Статус скрыт." }}
            </p>
          </div>
          <button
            class="primary"
            data-testid="presence-toggle"
            :title="presenceEnabled ? 'Скрыть музыку из профиля' : 'Показывать музыку в профиле'"
            @click="togglePresence"
          >
            {{ presenceEnabled ? "Выключить" : "Включить" }}
          </button>
        </div>
        <div v-for="toggle in PRESENCE_TOGGLES" :key="toggle.key" class="row">
          <div :title="toggle.tip">
            <p>{{ toggle.title }}</p>
            <p class="hint">{{ toggle.hint }}</p>
          </div>
          <label class="switch" :data-testid="toggle.testid" :title="toggle.tip ?? toggle.hint">
            <input
              type="checkbox"
              :checked="isEnabled(toggle.key)"
              :disabled="busy"
              :aria-label="toggle.title"
              @change="setToggle(toggle.key, ($event.target as HTMLInputElement).checked)"
            />
            <span class="slider" aria-hidden="true"></span>
          </label>
        </div>
      </div>

      <div class="card toggle-card">
        <h2 class="section-title">Приложение</h2>
        <div v-for="toggle in APP_TOGGLES" :key="toggle.key" class="row">
          <div>
            <p>{{ toggle.title }}</p>
            <p class="hint">{{ toggle.hint }}</p>
          </div>
          <label class="switch" :data-testid="toggle.testid" :title="toggle.hint">
            <input
              type="checkbox"
              :checked="isEnabled(toggle.key)"
              :disabled="busy"
              :aria-label="toggle.title"
              @change="setToggle(toggle.key, ($event.target as HTMLInputElement).checked)"
            />
            <span class="slider" aria-hidden="true"></span>
          </label>
        </div>
        <div class="row">
          <div :title="DIAG_TOGGLE.tip">
            <p>{{ DIAG_TOGGLE.title }}</p>
            <p class="hint">{{ DIAG_TOGGLE.hint }}</p>
          </div>
          <label
            class="switch"
            :data-testid="DIAG_TOGGLE.testid"
            :title="DIAG_TOGGLE.tip"
          >
            <input
              type="checkbox"
              :checked="isEnabled(DIAG_TOGGLE.key)"
              :disabled="busy"
              :aria-label="DIAG_TOGGLE.title"
              @change="setToggle(DIAG_TOGGLE.key, ($event.target as HTMLInputElement).checked)"
            />
            <span class="slider" aria-hidden="true"></span>
          </label>
        </div>
      </div>
    </div>

    <div class="col">
      <div class="card">
        <h2 class="section-title">Оформление</h2>
        <div class="row">
          <div>
            <p>Тема</p>
            <p class="hint">Как выглядит окно.</p>
          </div>
          <select
            class="theme-select"
            :value="themeValue()"
            :disabled="busy"
            data-testid="theme-select"
            title="Светлая, тёмная или как в системе"
            @change="setTheme(($event.target as HTMLSelectElement).value as AppSettings['theme'])"
          >
            <option value="system">Как в системе</option>
            <option value="light">Светлая</option>
            <option value="dark">Тёмная</option>
          </select>
        </div>
      </div>

      <div class="card">
        <h2 class="section-title">Откуда брать музыку</h2>
        <div class="row">
          <div title="Яндекс Музыка — по токену, точнее. Windows — без токена, из любого плеера">
            <p>Источник</p>
            <p class="hint">
              Авто: сначала Яндекс Музыка, иначе — другие плееры.
            </p>
          </div>
          <select
            class="theme-select"
            :value="sourceModeValue()"
            :disabled="busy"
            data-testid="source-mode-select"
            title="Какой источник использовать для названия трека"
            @change="setSourceMode(($event.target as HTMLSelectElement).value as AppSettings['sourceMode'])"
          >
            <option value="auto">Авто</option>
            <option value="ynison">Только Яндекс Музыка</option>
            <option value="media_control">Только Windows</option>
          </select>
        </div>
        <p
          class="hint footer"
          title="Режим Windows не показывает обложку и кнопку «Открыть»"
        >
          Без токена работает только режим Windows — без обложки.
        </p>
      </div>

      <div class="card">
        <h2 class="section-title">Выбор приложения</h2>
        <div class="row">
          <div title="Если статус прыгает между браузером и плеером — зафиксируйте нужный">
            <p>Слушать только…</p>
            <p class="hint">
              Нужно, только если статус прыгает между приложениями.
            </p>
          </div>
          <select
            class="theme-select"
            :value="smtcSourceAppValue()"
            :disabled="busy"
            data-testid="smtc-source-select"
            title="Игнорировать все приложения, кроме выбранного"
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
                  ? `Сейчас играет в приложениях: ${mediaSessions.length}.`
                  : "Пока ничего не играет — включите музыку и обновите."
              }}
            </p>
          </div>
          <button
            :disabled="sessionsLoading"
            data-testid="smtc-refresh"
            :title="'Показать, какие приложения сейчас играют'"
            @click="refreshSessions"
          >
            {{ sessionsLoading ? "Ищу…" : "Обновить" }}
          </button>
        </div>
        <details v-if="mediaSessions.length" class="session-details">
          <summary class="hint">Показать список приложений</summary>
          <ul class="session-list">
            <li
              v-for="session in mediaSessions"
              :key="session.appId + session.title"
              class="session-row"
              :title="sessionDebugLine(session)"
            >
              <strong>{{ session.appId }}</strong>
              <span v-if="session.isCurrent" class="badge">сейчас</span>
              <span class="hint">{{ session.title }}</span>
            </li>
          </ul>
        </details>
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

.session-details {
  margin-top: 8px;
}

.session-details summary {
  cursor: pointer;
  user-select: none;
  /* Нативный маркер прячем: его положение от текста средствами CSS
     не двигается — рисуем свою точку, которой можно управлять. */
  list-style: none;
  position: relative;
}

.session-details summary::-webkit-details-marker {
  display: none;
}

.session-details summary::marker {
  content: "";
}

/* Своя точка висит в левом паддинге карточки (18px): текст summary
   выровнен со строкой выше, а точка не касается рамки. */
.session-details summary::before {
  content: "";
  position: absolute;
  left: -12px;
  top: 50%;
  transform: translateY(-50%);
  width: 6px;
  height: 6px;
  border-radius: 50%;
  background: var(--text-muted);
}

.session-list {
  list-style: none;
  margin: 8px 0 0;
  padding: 0;
  display: flex;
  flex-direction: column;
  gap: 8px;
}

.session-row {
  display: flex;
  flex-wrap: wrap;
  align-items: baseline;
  gap: 4px 8px;
  padding: 8px 10px;
  border: 1px solid var(--border);
  border-radius: 8px;
}

.badge {
  font-size: 11px;
  font-weight: 700;
  padding: 1px 8px;
  border-radius: 999px;
  background: var(--accent);
  color: var(--accent-contrast);
}

.footer {
  margin-top: 10px;
}
</style>

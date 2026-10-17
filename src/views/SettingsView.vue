<script setup lang="ts">
import { computed, ref } from "vue";
import appStore from "../stores/appStore";
import type { AppSettings } from "../types/app";
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
    hint: "Подробные технические логи. Секреты не логируются никогда.",
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

.footer {
  margin-top: 10px;
}
</style>

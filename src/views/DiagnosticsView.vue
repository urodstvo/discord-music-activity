<script setup lang="ts">
import { computed, onMounted, reactive, ref } from "vue";
import appStore from "../stores/appStore";
import type { DiagnosticsReport } from "../types/app";
import { DISCORD_LABELS, YNISON_LABELS } from "../types/app";
import {
  checkForUpdates,
  downloadAndInstallUpdate,
  initialUpdaterState,
} from "../lib/updater";

const report = ref<DiagnosticsReport | null>(null);
const updater = reactive(initialUpdaterState());

onMounted(async () => {
  try {
    report.value = await appStore.getDiagnostics();
  } catch {
    appStore.pushLocalToast("error", "Не удалось получить диагностику");
  }
});

const ynisonLabel = computed(() => {
  const raw = report.value?.ynisonStatus;
  if (!raw) return "—";
  return YNISON_LABELS[raw as keyof typeof YNISON_LABELS] ?? raw;
});

const discordLabel = computed(() => {
  const raw = report.value?.discordStatus;
  if (!raw) return "—";
  return DISCORD_LABELS[raw as keyof typeof DISCORD_LABELS] ?? raw;
});

async function openLogs(): Promise<void> {
  try {
    await appStore.openLogDirectory();
  } catch {
    appStore.pushLocalToast("error", "Не удалось открыть папку логов");
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

async function copyReport(): Promise<void> {
  if (!report.value) return;
  const safe = {
    app: report.value.appVersion,
    ynison: ynisonLabel.value,
    discord: discordLabel.value,
    error: report.value.lastError
      ? `${report.value.lastError.message} (${formatDate(report.value.lastError.atUnixMs)})`
      : "нет",
  };
  const text = `Яндекс Музыка Presence — диагностика\nПриложение: ${safe.app}\nЯндекс Музыка: ${safe.ynison}\nDiscord: ${safe.discord}\nОшибка: ${safe.error}\n(токены и секреты в отчёт не входят)`;
  try {
    await navigator.clipboard.writeText(text);
    appStore.pushLocalToast("info", "Отчёт скопирован — секретов в нём нет");
  } catch {
    appStore.pushLocalToast("error", "Не удалось скопировать отчёт");
  }
}

async function quit(): Promise<void> {
  try {
    await appStore.quitApp();
  } catch {
    appStore.pushLocalToast("error", "Не удалось завершить приложение");
  }
}

const updaterStatusText = computed(() => {
  switch (updater.stage) {
    case "idle":
      return "Нажмите «Проверить», чтобы узнать о новой версии.";
    case "checking":
      return "Проверяю обновления…";
    case "available":
      return updater.latestVersion
        ? `Доступна версия ${updater.latestVersion}.`
        : "Доступна новая версия.";
    case "downloading":
      return updater.progress != null
        ? `Загрузка… ${updater.progress}%`
        : "Загрузка…";
    case "installing":
      return "Установка… приложение перезапустится.";
    case "ready":
      return "Обновление установлено.";
    case "up-to-date":
      return "Установлена последняя версия.";
    case "error":
      return updater.error ?? "Не удалось проверить обновления.";
  }
});

async function onCheckUpdates(): Promise<void> {
  const found = await checkForUpdates(updater);
  if (found) {
    appStore.pushLocalToast("info", `Доступна версия ${found}`);
  } else if (updater.stage === "up-to-date") {
    appStore.pushLocalToast("info", "Установлена последняя версия");
  } else if (updater.stage === "error") {
    appStore.pushLocalToast("error", updater.error ?? "Ошибка проверки обновлений");
  }
}

async function onInstallUpdate(): Promise<void> {
  await downloadAndInstallUpdate(updater);
  if (updater.stage === "error") {
    appStore.pushLocalToast("error", updater.error ?? "Ошибка установки обновления");
  }
}

function formatDate(ms: number): string {
  return new Date(ms).toLocaleString();
}
</script>

<template>
  <div class="view">
    <div class="card">
      <h2 class="section-title">Версии</h2>
      <div class="grid">
        <p class="hint">Приложение</p>
        <p>{{ report?.appVersion ?? "—" }}</p>
        <p class="hint">Tauri / WebView</p>
        <p>{{ report?.tauriVersion ?? "—" }}</p>
      </div>
    </div>

    <div class="card">
      <h2 class="section-title">Обновления</h2>
      <p class="hint" data-testid="updater-status">{{ updaterStatusText }}</p>
      <p v-if="updater.notes" class="notes">{{ updater.notes }}</p>
      <progress
        v-if="updater.stage === 'downloading' && updater.progress != null"
        :value="updater.progress"
        max="100"
      />
      <div class="actions">
        <button
          class="btn"
          title="Проверить наличие новой версии"
          :disabled="updater.stage === 'checking' || updater.stage === 'downloading' || updater.stage === 'installing'"
          @click="onCheckUpdates"
        >
          Проверить
        </button>
        <button
          v-if="updater.stage === 'available'"
          class="primary"
          title="Скачать и установить новую версию"
          @click="onInstallUpdate"
        >
          Установить {{ updater.latestVersion }}
        </button>
      </div>
    </div>

    <div class="card">
      <h2 class="section-title">Соединения</h2>
      <div class="grid">
        <p class="hint" title="Получение текущего трека из Яндекс Музыки">Яндекс Музыка</p>
        <p data-testid="diag-ynison">{{ ynisonLabel }}</p>
        <p class="hint" title="Публикация статуса в запущенном Discord">Discord</p>
        <p data-testid="diag-discord">{{ discordLabel }}</p>
      </div>
      <p
        v-if="report?.lastError"
        class="error-block"
        data-testid="diag-error"
        :title="`Код: ${report.lastError.code}`"
      >
        {{ report.lastError.message }}
        <span class="hint">({{ formatDate(report.lastError.atUnixMs) }})</span>
      </p>
      <p v-else class="hint">Ошибок нет — всё работает.</p>
    </div>

    <div class="card">
      <h2 class="section-title">Частые неполадки</h2>
      <div class="row">
        <div title="Если Discord был запущен после приложения">
          <p>Нет связи с Discord?</p>
          <p class="hint">Например, Discord запустился позже.</p>
        </div>
        <button
          title="Заново найти запущенный Discord"
          @click="reconnectDiscord"
        >
          Найти Discord
        </button>
      </div>
      <div class="row">
        <div title="Почему другие могут не видеть активность">
          <p>Друзья не видят активность?</p>
          <p class="hint">
            Кнопки видны только чужим, но не тебе. Если не видна и сама
            активность — проверь в Discord: статус не «Невидимка»,
            «Конфиденциальность активности» разрешает показ, а смотрят с
            полного профиля (друг или общий сервер).
          </p>
        </div>
      </div>
    </div>

    <div class="card">
      <h2 class="section-title">Поддержка</h2>
      <p
        class="hint log-path"
        data-testid="log-dir"
        :title="report?.logDir ?? ''"
      >
        {{ report?.logDir ?? "—" }}
      </p>
      <p class="hint" title="Токен и секреты в логи и отчёт не попадают">
        Логи и отчёт без секретов — их можно безопасно отправлять.
      </p>
      <div class="actions">
        <button
          class="primary"
          title="Открыть папку с файлами логов"
          @click="openLogs"
        >
          Папка логов
        </button>
        <button
          class="btn"
          title="Скопировать краткий отчёт без секретов"
          @click="copyReport"
        >
          Скопировать отчёт
        </button>
      </div>
    </div>

    <div class="card danger-zone">
      <h2 class="section-title">Выход</h2>
      <p class="hint">
        Приложение закроется, статус в Discord пропадёт.
      </p>
      <button
        class="danger"
        data-testid="quit"
        title="Остановить трансляцию и закрыть приложение"
        @click="quit"
      >
        Выйти из приложения
      </button>
    </div>
  </div>
</template>

<style scoped>
.grid {
  display: grid;
  grid-template-columns: auto 1fr;
  gap: 6px 16px;
  align-items: baseline;
}

.error-block {
  margin-top: 12px;
  color: var(--danger);
}

.log-path {
  margin-bottom: 8px;
  word-break: break-all;
  color: var(--text);
}

.actions {
  display: flex;
  align-items: center;
  flex-wrap: wrap;
  gap: 10px;
  margin-top: 10px;
}

.notes {
  margin-top: 8px;
  white-space: pre-wrap;
  word-break: break-word;
}

progress {
  width: 100%;
  margin-top: 8px;
}

.danger-zone button {
  margin-top: 8px;
}
</style>

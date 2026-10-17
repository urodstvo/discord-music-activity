<script setup lang="ts">
import { onMounted, ref } from "vue";
import appStore from "../stores/appStore";
import type { DiagnosticsReport } from "../types/app";

const report = ref<DiagnosticsReport | null>(null);
const expanded = ref(false);

onMounted(async () => {
  try {
    report.value = await appStore.getDiagnostics();
  } catch {
    appStore.pushLocalToast("error", "Не удалось получить диагностику");
  }
});

async function openLogs(): Promise<void> {
  try {
    await appStore.openLogDirectory();
  } catch {
    appStore.pushLocalToast("error", "Не удалось открыть папку логов");
  }
}

async function quit(): Promise<void> {
  try {
    await appStore.quitApp();
  } catch {
    appStore.pushLocalToast("error", "Не удалось завершить приложение");
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
      <h2 class="section-title">Состояние соединений</h2>
      <div class="grid">
        <p class="hint">Яндекс Музыка</p>
        <p data-testid="diag-ynison">{{ report?.ynisonStatus ?? "—" }}</p>
        <p class="hint">Discord</p>
        <p data-testid="diag-discord">{{ report?.discordStatus ?? "—" }}</p>
      </div>
      <p v-if="report?.lastError" class="error-block" data-testid="diag-error">
        Последняя ошибка: <strong>{{ report.lastError.code }}</strong> —
        {{ report.lastError.message }}
        <span class="hint">({{ formatDate(report.lastError.atUnixMs) }})</span>
      </p>
      <p v-else class="hint">Критичных ошибок не зафиксировано.</p>
    </div>

    <div class="card">
      <h2 class="section-title">Журнал</h2>
      <p class="hint log-path" data-testid="log-dir">
        {{ report?.logDir ?? "—" }}
      </p>
      <p class="hint">
        Логи пишутся на диск с ротацией по дням; OAuth-токен и секретные
        заголовки в лог не попадают.
      </p>
      <div class="actions">
        <button class="primary" @click="openLogs">Открыть папку логов</button>
        <span v-if="expanded" class="hint">
          Автоматическая отправка логов разработчику не выполняется.
        </span>
        <button class="btn" @click="expanded = !expanded">
          {{ expanded ? "Свернуть" : "Подробнее" }}
        </button>
      </div>
    </div>

    <div class="card danger-zone">
      <h2 class="section-title">Завершение</h2>
      <p class="hint">
        Полный выход останавливает синхронизацию и очищает Discord Activity.
      </p>
      <button class="danger" data-testid="quit" @click="quit">Выйти из приложения</button>
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

.danger-zone button {
  margin-top: 8px;
}
</style>

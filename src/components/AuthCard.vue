<script setup lang="ts">
import { computed, onUnmounted, ref } from "vue";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { openUrl } from "@tauri-apps/plugin-opener";
import appStore from "../stores/appStore";
import type { DeviceCodeInfo } from "../types/app";

const authed = computed(
  () => appStore.state.snapshot !== null && !appStore.state.snapshot.authRequired,
);

// --- Вход через Яндекс (Device Flow) ---
const starting = ref(false);
const waiting = ref(false);
const expired = ref(false);
const deviceInfo = ref<DeviceCodeInfo | null>(null);
const secondsLeft = ref(0);
let countdown: ReturnType<typeof setInterval> | null = null;
let unlisteners: UnlistenFn[] = [];

function stopCountdown(): void {
  if (countdown !== null) {
    clearInterval(countdown);
    countdown = null;
  }
}

function resetDeviceState(): void {
  stopCountdown();
  for (const stop of unlisteners) stop();
  unlisteners = [];
  waiting.value = false;
  deviceInfo.value = null;
  secondsLeft.value = 0;
}

onUnmounted(() => {
  resetDeviceState();
});

async function startLogin(): Promise<void> {
  if (starting.value || waiting.value) return;
  starting.value = true;
  expired.value = false;
  try {
    const info = await appStore.startDeviceAuth();
    deviceInfo.value = info;
    waiting.value = true;
    secondsLeft.value = info.expiresInSecs;
    stopCountdown();
    countdown = setInterval(() => {
      secondsLeft.value -= 1;
      if (secondsLeft.value <= 0) {
        // Сервер пришлёт device-auth-error почти одновременно; здесь только
        // сбрасываем UI без лишнего тоста (уведомление уже идёт из backend).
        resetDeviceState();
        expired.value = true;
      }
    }, 1000);

    const done = await listen("device-auth-done", () => {
      // Успешный тост уже отправлен из backend — локальный не нужен,
      // иначе будет двойная плашка (как раньше с удалением токена).
      resetDeviceState();
    });
    const failed = await listen("device-auth-error", () => {
      // Текст ошибки уже показан через notification из backend.
      resetDeviceState();
    });
    unlisteners = [done, failed];
  } catch (err) {
    appStore.pushLocalToast("error", describe(err, "Не удалось начать вход"));
  } finally {
    starting.value = false;
  }
}

async function cancelLogin(): Promise<void> {
  try {
    await appStore.cancelDeviceAuth();
  } catch {
    // Отмена best-effort: даже при ошибке команды просто сбрасываем UI.
  }
  resetDeviceState();
}

async function openVerificationPage(): Promise<void> {
  const url = deviceInfo.value?.verificationUrl;
  if (!url) return;
  try {
    await openUrl(url);
  } catch {
    appStore.pushLocalToast("info", "Откройте ссылку вручную — она ниже");
  }
}

async function copyCode(): Promise<void> {
  const code = deviceInfo.value?.userCode;
  if (!code) return;
  try {
    await navigator.clipboard.writeText(code);
    appStore.pushLocalToast("info", "Код скопирован");
  } catch {
    appStore.pushLocalToast("error", "Не удалось скопировать код");
  }
}

function fmtCountdown(total: number): string {
  const safe = Math.max(0, total);
  return `${Math.floor(safe / 60)}:${String(safe % 60).padStart(2, "0")}`;
}

// --- Ручной ввод токена убран из UI: вход только через Яндекс (Device Flow).
// Backend-команды save/validate оставлены для совместимости Tauri API.
const busy = ref(false);

function describe(err: unknown, fallback: string): string {
  if (typeof err === "object" && err !== null && "message" in err) {
    return String((err as { message: unknown }).message);
  }
  return fallback;
}

async function removeToken(): Promise<void> {
  busy.value = true;
  try {
    await appStore.removeToken();
    // Успешный тост («Токен удалён») уже отправлен из backend через
    // notification — локальный приводил к двойной плашке.
  } catch (err) {
    appStore.pushLocalToast("error", describe(err, "Не удалось удалить токен"));
  } finally {
    busy.value = false;
  }
}
</script>

<template>
  <div class="card auth-card" data-testid="auth-card">
    <h2 class="section-title">Аккаунт Яндекс Музыки</h2>

    <template v-if="authed">
      <p class="hint" title="Токен хранится в Credential Manager и никуда не отправляется, кроме Яндекс Музыки">
        Подключено. Токен надёжно сохранён.
      </p>
      <div class="actions">
        <button
          class="danger"
          :disabled="busy"
          title="Отключить аккаунт и остановить трансляцию"
          @click="removeToken"
        >
          Отключить аккаунт
        </button>
      </div>
    </template>

    <template v-else-if="waiting && deviceInfo">
      <p class="hint">Введите этот код на странице Яндекса:</p>
      <div class="code-row">
        <code class="user-code" data-testid="device-user-code" :title="'Код: ' + deviceInfo.userCode">
          {{ deviceInfo.userCode }}
        </code>
        <button class="btn" type="button" title="Скопировать код" @click="copyCode">
          Копировать
        </button>
      </div>
      <div class="actions">
        <button
          class="primary"
          type="button"
          title="Открыть страницу ввода кода в браузере"
          @click="openVerificationPage"
        >
          Открыть страницу Яндекса
        </button>
        <button class="btn" type="button" title="Отменить вход" @click="cancelLogin">
          Отмена
        </button>
      </div>
      <p class="hint">
        Жду подтверждения… (осталось {{ fmtCountdown(secondsLeft) }}).
        После разрешения доступа токен сохранится сам.
      </p>
      <p class="hint url" :title="deviceInfo.verificationUrl">
        {{ deviceInfo.verificationUrl }}
      </p>
    </template>

    <template v-else>
      <p class="hint" title="Токен сохранится только в системном хранилище Windows и не попадёт в логи">
        Войдите через Яндекс — токен подхватится сам, ничего копировать не нужно.
      </p>
      <div class="actions">
        <button
          class="primary"
          data-testid="device-login"
          :disabled="starting"
          title="Показать код и открыть страницу подтверждения Яндекса"
          @click="startLogin"
        >
          {{ starting ? "Готовлю код…" : "Войти через Яндекс" }}
        </button>
      </div>
      <p v-if="expired" class="hint expired">
        Код истёк — нажмите «Войти через Яндекс» ещё раз.
      </p>
    </template>
  </div>
</template>

<style scoped>
.actions {
  display: flex;
  gap: 10px;
  margin-top: 14px;
}

.code-row {
  display: flex;
  align-items: center;
  gap: 10px;
  margin: 10px 0 4px;
}

.user-code {
  font-size: 24px;
  font-weight: 700;
  letter-spacing: 2px;
  padding: 8px 14px;
  border: 1px dashed var(--border);
  border-radius: 8px;
  background: var(--bg-input);
  user-select: all;
}

.url {
  word-break: break-all;
  user-select: all;
}

.expired {
  color: var(--warning);
}

.manual {
  margin-top: 14px;
}

.manual summary {
  cursor: pointer;
  user-select: none;
}

.manual label {
  display: block;
  margin-top: 10px;
}
</style>

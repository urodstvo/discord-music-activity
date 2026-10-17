<script setup lang="ts">
import { computed, ref } from "vue";
import appStore from "../stores/appStore";

/** Поле ввода токена: protected input — по умолчанию скрыт (ТЗ §5.1). */
const tokenInput = ref("");
const showToken = ref(false);
const busy = ref(false);

const authed = computed(
  () => appStore.state.snapshot !== null && !appStore.state.snapshot.authRequired,
);

function describe(err: unknown, fallback: string): string {
  if (typeof err === "object" && err !== null && "message" in err) {
    return String((err as { message: unknown }).message);
  }
  return fallback;
}

async function saveToken(): Promise<void> {
  if (!tokenInput.value.trim()) {
    appStore.pushLocalToast("warning", "Введите OAuth-токен");
    return;
  }
  busy.value = true;
  try {
    await appStore.saveToken(tokenInput.value);
    tokenInput.value = "";
    showToken.value = false;
    appStore.pushLocalToast("info", "Токен сохранён");
  } catch (err) {
    appStore.pushLocalToast("error", describe(err, "Не удалось сохранить токен"));
  } finally {
    busy.value = false;
  }
}

async function removeToken(): Promise<void> {
  busy.value = true;
  try {
    await appStore.removeToken();
    appStore.pushLocalToast("info", "Токен удалён");
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
      <p class="hint">
        Токен сохранён в системном хранилище Windows (Credential Manager).
        Синхронизация Playback активна.
      </p>
      <div class="actions">
        <button class="danger" :disabled="busy" @click="removeToken">
          Удалить токен
        </button>
      </div>
    </template>

    <template v-else>
      <p class="hint">
        Вставьте OAuth-токен Яндекс Музыки. Он хранится только в Windows
        Credential Manager и не попадает в конфиги и логи.
      </p>
      <label for="token-input">OAuth-токен</label>
      <div class="token-row">
        <input
          id="token-input"
          v-model="tokenInput"
          :type="showToken ? 'text' : 'password'"
          placeholder="y0_AgAAAA…"
          autocomplete="off"
          spellcheck="false"
          @keydown.enter="saveToken"
        />
        <button class="btn" type="button" @click="showToken = !showToken">
          {{ showToken ? "Скрыть" : "Показать" }}
        </button>
      </div>
      <div class="actions">
        <button class="primary" :disabled="busy" @click="saveToken">
          Сохранить токен
        </button>
      </div>
      <p class="hint where">
        Где взять токен: откройте
        <a
          href="https://music.yandex.ru"
          target="_blank"
          rel="noreferrer noopener"
          >music.yandex.ru</a
        >
        и используйте значение из cookie <code>YandexMusic OAuth</code> либо
        инструкцию проекта.
      </p>
    </template>
  </div>
</template>

<style scoped>
.token-row {
  display: flex;
  gap: 8px;
}

.token-row input {
  flex: 1;
  min-width: 0;
}

.actions {
  display: flex;
  gap: 10px;
  margin-top: 14px;
}

.where {
  margin-top: 12px;
}

.where a {
  color: var(--accent);
  text-decoration: underline;
}
</style>

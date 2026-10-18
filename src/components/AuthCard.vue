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

    <template v-else>
      <p class="hint" title="Токен хранится только в системном хранилище Windows и не попадает в логи">
        Нужен для трансляции треков из Яндекс Музыки.
      </p>
      <label for="token-input">Токен Яндекс Музыки</label>
      <div class="token-row">
        <input
          id="token-input"
          v-model="tokenInput"
          :type="showToken ? 'text' : 'password'"
          placeholder="Вставьте токен…"
          autocomplete="off"
          spellcheck="false"
          title="OAuth-токен Яндекс Музыки — никому его не показывайте"
          @keydown.enter="saveToken"
        />
        <button
          class="btn"
          type="button"
          :title="showToken ? 'Скрыть токен' : 'Показать токен'"
          @click="showToken = !showToken"
        >
          {{ showToken ? "Скрыть" : "Показать" }}
        </button>
      </div>
      <div class="actions">
        <button
          class="primary"
          :disabled="busy"
          title="Проверить и сохранить токен"
          @click="saveToken"
        >
          Подключить
        </button>
      </div>
      <p class="hint where">
        Где взять: откройте
        <a
          href="https://music.yandex.ru"
          target="_blank"
          rel="noreferrer noopener"
          title="Открыть Яндекс Музыку в браузере"
          >music.yandex.ru</a
        >
        и скопируйте токен из cookie — подробности в инструкции проекта.
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

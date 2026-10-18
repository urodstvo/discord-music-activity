<script setup lang="ts">
import appStore from "../stores/appStore";
import {
  YNISON_LABELS,
  DISCORD_LABELS,
  SOURCE_LABELS,
} from "../types/app";

function tone(value: string | undefined): "ok" | "warn" | "bad" {
  switch (value) {
    case "connected":
      return "ok";
    case "connecting":
    case "reconnecting":
      return "warn";
    default:
      return "bad";
  }
}

const ynisonLabel = (): string => {
  const status = appStore.state.snapshot?.ynison;
  return status ? (YNISON_LABELS[status] ?? status) : "—";
};

const discordLabel = (): string => {
  const status = appStore.state.snapshot?.discord;
  return status ? (DISCORD_LABELS[status] ?? status) : "—";
};

const sourceLabel = (): string => {
  const source = appStore.state.snapshot?.source;
  return source ? (SOURCE_LABELS[source] ?? source) : "—";
};
</script>

<template>
  <div class="connections">
    <div class="chip">
      <span class="dot" :class="tone(appStore.state.snapshot?.ynison)" aria-hidden="true"></span>
      <span>Яндекс Музыка: <strong>{{ ynisonLabel() }}</strong></span>
    </div>
    <div class="chip">
      <span class="dot" :class="tone(appStore.state.snapshot?.discord)" aria-hidden="true"></span>
      <span>Discord: <strong>{{ discordLabel() }}</strong></span>
    </div>
    <div
      v-if="appStore.state.snapshot?.source"
      class="chip"
      data-testid="active-source"
    >
      <span class="dot warn" aria-hidden="true"></span>
      <span>Источник данных: <strong>{{ sourceLabel() }}</strong></span>
    </div>
    <button
      v-if="appStore.state.snapshot?.lastError"
      class="btn"
      @click="appStore.reconnectYandex()"
    >
      Повторить подключение
    </button>
  </div>
</template>

<style scoped>
.connections {
  display: flex;
  flex-direction: column;
  gap: 10px;
}

.chip {
  display: flex;
  align-items: center;
  gap: 10px;
  font-size: 13px;
}

.dot {
  width: 10px;
  height: 10px;
  border-radius: 50%;
  background: var(--text-muted);
}

.dot.ok {
  background: var(--success);
}

.dot.warn {
  background: var(--warning);
}

.dot.bad {
  background: var(--danger);
}
</style>

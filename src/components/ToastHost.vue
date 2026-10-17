<script setup lang="ts">
import appStore from "../stores/appStore";

function dismiss(id: number): void {
  appStore.state.toasts = appStore.state.toasts.filter((t) => t.id !== id);
}
</script>

<template>
  <div class="toasts" aria-live="polite">
    <transition-group name="toast">
      <div
        v-for="toast in appStore.state.toasts"
        :key="toast.id"
        class="toast"
        :class="toast.level"
        role="status"
        @click="dismiss(toast.id)"
      >
        {{ toast.message }}
      </div>
    </transition-group>
  </div>
</template>

<style scoped>
.toasts {
  position: fixed;
  right: 16px;
  bottom: 16px;
  display: flex;
  flex-direction: column;
  gap: 8px;
  z-index: 50;
  max-width: 340px;
}

.toast {
  padding: 10px 14px;
  border-radius: var(--radius);
  background: var(--bg-elevated);
  border: 1px solid var(--border);
  box-shadow: var(--shadow);
  font-size: 13px;
  cursor: pointer;
  border-left: 3px solid var(--accent);
}

.toast.error {
  border-left-color: var(--danger);
}

.toast.warning {
  border-left-color: var(--warning);
}

.toast.info {
  border-left-color: var(--success);
}

.toast-enter-active,
.toast-leave-active {
  transition: opacity 0.2s ease, transform 0.2s ease;
}

.toast-enter-from,
.toast-leave-to {
  opacity: 0;
  transform: translateY(6px);
}
</style>

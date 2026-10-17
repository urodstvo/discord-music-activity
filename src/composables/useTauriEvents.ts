// Безопасная обёртка над Tauri events: один подписчик на событие,
// корректная отписка при уничтожении компонента (ТЗ §10.2).

import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { onMounted, onUnmounted } from "vue";

export function useTauriEvent<T>(
  eventName: string,
  handler: (payload: T) => void,
): void {
  let unlisten: UnlistenFn | null = null;

  onMounted(async () => {
    unlisten = await listen<T>(eventName, (event) => {
      handler(event.payload);
    });
  });

  onUnmounted(() => {
    unlisten?.();
    unlisten = null;
  });
}

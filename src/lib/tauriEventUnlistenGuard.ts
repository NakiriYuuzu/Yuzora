import { isTauri } from "@tauri-apps/api/core";

const guardMarker = Symbol.for("yuzora.tauri-event-unlisten-guard");

function isMissingListenerTypeError(error: unknown): error is TypeError {
  return (
    error instanceof TypeError &&
    (error.message ===
      "undefined is not an object (evaluating 'listeners[eventId].handlerId')" ||
      error.message === "Cannot read properties of undefined (reading 'handlerId')")
  );
}

export function installTauriEventUnlistenGuard(): void {
  if (!isTauri()) return;

  const eventInternals = window.__TAURI_EVENT_PLUGIN_INTERNALS__;
  const unregisterListener = eventInternals?.unregisterListener;

  if (
    typeof unregisterListener !== "function" ||
    Reflect.get(unregisterListener, guardMarker) === true
  ) {
    return;
  }

  const guardedUnregisterListener = function (
    this: unknown,
    ...args: Parameters<typeof unregisterListener>
  ): void {
    try {
      Reflect.apply(unregisterListener, this, args);
    } catch (error) {
      // Tauri 2.11.5 reload race: a stale ID can be absent from the new document's listener map.
      if (!isMissingListenerTypeError(error)) throw error;
    }
  };

  Reflect.defineProperty(guardedUnregisterListener, guardMarker, { value: true });
  eventInternals.unregisterListener = guardedUnregisterListener;
}

import { isTauri } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { clearMocks, mockIPC } from "@tauri-apps/api/mocks";
import { afterEach, describe, expect, it, vi } from "vitest";

import { installTauriEventUnlistenGuard } from "./tauriEventUnlistenGuard";

interface InvokeCall {
  command: string;
  payload: unknown;
}

const SAFARI_MISSING_LISTENER_ERROR =
  "undefined is not an object (evaluating 'listeners[eventId].handlerId')";
const CHROMIUM_MISSING_LISTENER_ERROR =
  "Cannot read properties of undefined (reading 'handlerId')";

function mockTauriEventIpc(eventId: number): InvokeCall[] {
  const calls: InvokeCall[] = [];

  Object.defineProperty(globalThis, "isTauri", {
    configurable: true,
    value: true,
  });
  mockIPC((command, payload) => {
    calls.push({ command, payload });
    if (command === "plugin:event|listen") return eventId;
    return undefined;
  });

  return calls;
}

function backendUnlistenCalls(calls: InvokeCall[]): InvokeCall[] {
  return calls.filter(({ command }) => command === "plugin:event|unlisten");
}

afterEach(() => {
  clearMocks();
  Reflect.deleteProperty(window, "__TAURI_EVENT_PLUGIN_INTERNALS__");
  Reflect.deleteProperty(window, "__TAURI_INTERNALS__");
  Reflect.deleteProperty(globalThis, "isTauri");
  vi.restoreAllMocks();
});

describe("installTauriEventUnlistenGuard", () => {
  it("shows the unguarded Tauri behavior: a missing listener prevents backend unlisten", async () => {
    const calls = mockTauriEventIpc(41);
    const unlisten = await listen("workspace-updated", vi.fn());
    const error = new TypeError(SAFARI_MISSING_LISTENER_ERROR);
    window.__TAURI_EVENT_PLUGIN_INTERNALS__.unregisterListener = vi.fn(() => {
      throw error;
    });

    await expect(unlisten()).rejects.toBe(error);
    expect(backendUnlistenCalls(calls)).toEqual([]);
  });

  it("continues backend unlisten after the Safari missing-handlerId TypeError", async () => {
    const calls = mockTauriEventIpc(42);
    window.__TAURI_EVENT_PLUGIN_INTERNALS__.unregisterListener = vi.fn(() => {
      throw new TypeError(SAFARI_MISSING_LISTENER_ERROR);
    });
    installTauriEventUnlistenGuard();

    const unlisten = await listen("workspace-updated", vi.fn());

    await expect(unlisten()).resolves.toBeUndefined();
    expect(backendUnlistenCalls(calls)).toEqual([
      {
        command: "plugin:event|unlisten",
        payload: { event: "workspace-updated", eventId: 42 },
      },
    ]);
  });

  it("continues backend unlisten after the Chromium missing-handlerId TypeError", async () => {
    const calls = mockTauriEventIpc(43);
    window.__TAURI_EVENT_PLUGIN_INTERNALS__.unregisterListener = vi.fn(() => {
      throw new TypeError(CHROMIUM_MISSING_LISTENER_ERROR);
    });
    installTauriEventUnlistenGuard();

    const unlisten = await listen("terminal-output", vi.fn());

    await expect(unlisten()).resolves.toBeUndefined();
    expect(backendUnlistenCalls(calls)).toEqual([
      {
        command: "plugin:event|unlisten",
        payload: { event: "terminal-output", eventId: 43 },
      },
    ]);
  });

  it("rethrows unrelated TypeErrors without invoking backend unlisten", async () => {
    const calls = mockTauriEventIpc(44);
    const error = new TypeError("handlerId must be a finite number");
    window.__TAURI_EVENT_PLUGIN_INTERNALS__.unregisterListener = vi.fn(() => {
      throw error;
    });
    installTauriEventUnlistenGuard();

    const unlisten = await listen("workspace-updated", vi.fn());

    await expect(unlisten()).rejects.toBe(error);
    expect(backendUnlistenCalls(calls)).toEqual([]);
  });

  it("preserves normal unregister behavior, this, arguments, and idempotent installation", async () => {
    const calls = mockTauriEventIpc(45);
    const internals = window.__TAURI_EVENT_PLUGIN_INTERNALS__;
    const contexts: unknown[] = [];
    const unregisterListener = vi.fn(function (
      this: unknown,
      _event: string,
      _eventId: number,
    ) {
      contexts.push(this);
    });
    internals.unregisterListener = unregisterListener;

    installTauriEventUnlistenGuard();
    const guardedUnregisterListener = internals.unregisterListener;
    installTauriEventUnlistenGuard();

    expect(internals.unregisterListener).toBe(guardedUnregisterListener);

    const unlisten = await listen("workspace-updated", vi.fn());
    await unlisten();

    expect(unregisterListener).toHaveBeenCalledOnce();
    expect(unregisterListener).toHaveBeenCalledWith("workspace-updated", 45);
    expect(contexts).toEqual([internals]);
    expect(backendUnlistenCalls(calls)).toEqual([
      {
        command: "plugin:event|unlisten",
        payload: { event: "workspace-updated", eventId: 45 },
      },
    ]);
  });

  it("leaves the existing unregister listener untouched outside Tauri", () => {
    mockIPC(() => undefined);
    const unregisterListener = window.__TAURI_EVENT_PLUGIN_INTERNALS__.unregisterListener;

    expect(isTauri()).toBe(false);
    installTauriEventUnlistenGuard();

    expect(window.__TAURI_EVENT_PLUGIN_INTERNALS__.unregisterListener).toBe(
      unregisterListener,
    );
  });

  it("is safe in Tauri when event internals are missing", () => {
    Object.defineProperty(globalThis, "isTauri", {
      configurable: true,
      value: true,
    });
    Reflect.deleteProperty(window, "__TAURI_EVENT_PLUGIN_INTERNALS__");

    expect(isTauri()).toBe(true);
    expect(() => installTauriEventUnlistenGuard()).not.toThrow();
  });
});

import "@testing-library/jest-dom/vitest";
import { afterEach } from "vitest";

import { previewInitialState, usePreviewStore } from "@/state/previewStore";
import { terminalInitialState, useTerminalStore } from "@/state/terminalStore";
import { uiInitialState, useUiStore } from "@/state/uiStore";

// jsdom doesn't implement these; cmdk (command palette) and Radix primitives
// touch them during layout/scroll measurement. Minimal no-op stand-ins.
class ResizeObserverMock {
  observe() {}
  unobserve() {}
  disconnect() {}
}
globalThis.ResizeObserver ??= ResizeObserverMock as unknown as typeof ResizeObserver;

if (!window.matchMedia) {
  window.matchMedia = (query: string) => ({
    matches: false,
    media: query,
    onchange: null,
    addListener: () => {},
    removeListener: () => {},
    addEventListener: () => {},
    removeEventListener: () => {},
    dispatchEvent: () => false,
  }) as unknown as MediaQueryList;
}

Element.prototype.scrollIntoView ??= () => {};

// jsdom doesn't implement pointer capture; resize-handle drags (nav width,
// terminal height) call these on pointerdown/pointerup.
Element.prototype.setPointerCapture ??= () => {};
Element.prototype.releasePointerCapture ??= () => {};
Element.prototype.hasPointerCapture ??= () => false;

// Reset theme state between tests so a failing/aborted theme test can't leak
// the `dark` class into unrelated tests that run after it.
afterEach(() => document.documentElement.classList.remove("dark"));

// zustand stores persist across the module graph, so mode / Git selection /
// resolver state set in one test leaks into the next. Reset to initial state.
afterEach(() => useUiStore.setState(uiInitialState));
afterEach(() => useTerminalStore.setState(terminalInitialState));
afterEach(() => usePreviewStore.setState(previewInitialState));

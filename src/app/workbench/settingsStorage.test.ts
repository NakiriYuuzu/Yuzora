import { beforeEach, describe, expect, it } from "vitest"

import {
  APPEARANCE_SETTINGS_STORAGE_KEY,
  loadAppearanceSettings,
  saveAppearanceSettings,
} from "./settingsStorage"

// The Bun-hosted test runtime injects an empty `localStorage` global with no
// Storage methods (see gitStore.test.ts). Install a minimal in-memory Storage
// so persistence is exercised for real.
function installLocalStorage(): void {
  const store = new Map<string, string>()
  const mock = {
    getItem: (k: string) => (store.has(k) ? store.get(k)! : null),
    setItem: (k: string, v: string) => void store.set(k, String(v)),
    removeItem: (k: string) => void store.delete(k),
    clear: () => store.clear(),
    key: (i: number) => [...store.keys()][i] ?? null,
    get length() {
      return store.size
    },
  }
  Object.defineProperty(globalThis, "localStorage", {
    value: mock,
    configurable: true,
    writable: true,
  })
}

beforeEach(() => {
  installLocalStorage()
  localStorage.clear()
})

describe("appearance settings", () => {
  it("沒有持久化值時回傳預設 auto", () => {
    expect(loadAppearanceSettings()).toEqual({ theme: "auto" })
  })

  it("壞 JSON 時回傳預設 auto", () => {
    localStorage.setItem(APPEARANCE_SETTINGS_STORAGE_KEY, "{not json")
    expect(loadAppearanceSettings()).toEqual({ theme: "auto" })
  })

  it("非法 theme 值時回傳預設 auto", () => {
    localStorage.setItem(APPEARANCE_SETTINGS_STORAGE_KEY, JSON.stringify({ theme: "neon" }))
    expect(loadAppearanceSettings()).toEqual({ theme: "auto" })
    localStorage.setItem(APPEARANCE_SETTINGS_STORAGE_KEY, JSON.stringify({ theme: 42 }))
    expect(loadAppearanceSettings()).toEqual({ theme: "auto" })
  })

  it("save→load 往返對三個合法值一致", () => {
    for (const theme of ["light", "dark", "auto"] as const) {
      saveAppearanceSettings({ theme })
      expect(loadAppearanceSettings()).toEqual({ theme })
    }
  })
})

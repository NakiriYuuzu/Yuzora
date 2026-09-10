import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"

import {
  APPEARANCE_SETTINGS_STORAGE_KEY,
  TERMINAL_SETTINGS_STORAGE_KEY,
  loadAppearanceSettings,
  loadTerminalSettings,
  saveAppearanceSettings,
  writeJsonSetting,
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
  vi.spyOn(navigator, "hardwareConcurrency", "get").mockReturnValue(8)
  Object.defineProperty(navigator, "deviceMemory", { configurable: true, value: 8 })
  localStorage.clear()
})

afterEach(() => {
  vi.restoreAllMocks()
  delete (navigator as Navigator & { deviceMemory?: number }).deviceMemory
})

describe("appearance settings", () => {
  it.each([[2, 8], [4, 8], [8, 4], [8, 2], [0, 8]])("defaults bot animations off for %i cores / %i GB", (cores, memory) => {
    vi.spyOn(navigator, "hardwareConcurrency", "get").mockReturnValue(cores)
    Object.defineProperty(navigator, "deviceMemory", { configurable: true, value: memory })
    expect(loadAppearanceSettings().botAnimations).toBe(false)
  })

  it("keeps animations available when memory reporting is unavailable on a capable device", () => {
    delete (navigator as Navigator & { deviceMemory?: number }).deviceMemory
    expect(loadAppearanceSettings().botAnimations).toBe(true)
  })

  it("defaults bot animations off for reduced motion", () => {
    const media = window.matchMedia("(prefers-reduced-motion: reduce)")
    vi.spyOn(window, "matchMedia").mockReturnValue({ ...media, matches: true })
    expect(loadAppearanceSettings().botAnimations).toBe(false)
  })

  it("persists explicit bot animation choices instead of replacing them with hardware defaults", () => {
    vi.spyOn(navigator, "hardwareConcurrency", "get").mockReturnValue(2)
    for (const botAnimations of [true, false]) {
      saveAppearanceSettings({ ...loadAppearanceSettings(), botAnimations })
      expect(loadAppearanceSettings().botAnimations).toBe(botAnimations)
    }
  })

  it("uses hardware defaults for missing or invalid animation preferences", () => {
    for (const botAnimations of [undefined, "false", 0, null]) {
      localStorage.setItem(APPEARANCE_SETTINGS_STORAGE_KEY, JSON.stringify({ botAnimations }))
      expect(loadAppearanceSettings().botAnimations).toBe(true)
    }
  })
  it("舊版設定預設開啟兩側背景，非 boolean 值不視為使用者偏好", () => {
    for (const sidebarFields of [{}, { leftSidebarBackground: "false", rightSidebarBackground: 0 }]) {
      localStorage.setItem(APPEARANCE_SETTINGS_STORAGE_KEY, JSON.stringify({ theme: "light", accent: "blue", ...sidebarFields }))
      expect(loadAppearanceSettings()).toEqual({ theme: "light", accent: "blue", leftSidebarBackground: true, rightSidebarBackground: true, botAnimations: true })
    }
  })

  it("沒有持久化值時回傳預設 auto 與 lime", () => {
    expect(loadAppearanceSettings()).toEqual({ theme: "auto", accent: "lime", leftSidebarBackground: true, rightSidebarBackground: true, botAnimations: true })
  })

  it("壞 JSON 時回傳預設 auto 與 lime", () => {
    localStorage.setItem(APPEARANCE_SETTINGS_STORAGE_KEY, "{not json")
    expect(loadAppearanceSettings()).toEqual({ theme: "auto", accent: "lime", leftSidebarBackground: true, rightSidebarBackground: true, botAnimations: true })
  })

  it("非法 theme 或 accent 值時分別回傳預設值", () => {
    localStorage.setItem(
      APPEARANCE_SETTINGS_STORAGE_KEY,
      JSON.stringify({ theme: "neon", accent: "infrared" })
    )
    expect(loadAppearanceSettings()).toEqual({ theme: "auto", accent: "lime", leftSidebarBackground: true, rightSidebarBackground: true, botAnimations: true })
    localStorage.setItem(
      APPEARANCE_SETTINGS_STORAGE_KEY,
      JSON.stringify({ theme: 42, accent: 42 })
    )
    expect(loadAppearanceSettings()).toEqual({ theme: "auto", accent: "lime", leftSidebarBackground: true, rightSidebarBackground: true, botAnimations: true })
  })

  it("不把 Object prototype inherited keys 當成合法 accent", () => {
    for (const accent of ["constructor", "toString"]) {
      localStorage.setItem(
        APPEARANCE_SETTINGS_STORAGE_KEY,
        JSON.stringify({ theme: "dark", accent }),
      )
      expect(loadAppearanceSettings()).toEqual({ theme: "dark", accent: "lime", leftSidebarBackground: true, rightSidebarBackground: true, botAnimations: true })
    }
  })

  it("save→load 往返保留合法 theme 與 accent", () => {
    for (const theme of ["light", "dark", "auto"] as const) {
      saveAppearanceSettings({ theme, accent: "violet", leftSidebarBackground: false, rightSidebarBackground: true, botAnimations: true })
      expect(loadAppearanceSettings()).toEqual({ theme, accent: "violet", leftSidebarBackground: false, rightSidebarBackground: true, botAnimations: true })
    }
  })
})

describe("terminal settings", () => {
  it("retains display preferences while ignoring removed shell profiles", () => {
    writeJsonSetting(TERMINAL_SETTINGS_STORAGE_KEY, {
      shellPath: "/bin/zsh", defaultProfile: { shell: "/bin/zsh" },
      fontSize: 18, fontFamily: "menlo", imeAnchorMode: "tui",
    })
    expect(loadTerminalSettings()).toEqual({ fontSize: 18, fontFamily: "menlo", imeAnchorMode: "tui", copyOnSelect: true })
    writeJsonSetting(TERMINAL_SETTINGS_STORAGE_KEY, { imeAnchorMode: "floating" })
    expect(loadTerminalSettings().imeAnchorMode).toBe("cursor")
  })

  it("normalizes terminal font size into the supported range", () => {
    writeJsonSetting(TERMINAL_SETTINGS_STORAGE_KEY, { fontSize: 99 })
    expect(loadTerminalSettings().fontSize).toBe(32)

    writeJsonSetting(TERMINAL_SETTINGS_STORAGE_KEY, { fontSize: "large" })
    expect(loadTerminalSettings().fontSize).toBe(12)
  })
})

describe("terminal font preference migration", () => {
  it("defaults selection copy on and persists an explicit off switch", async () => {
    expect(loadTerminalSettings().copyOnSelect).toBe(true)
    const { useTerminalSettingsStore, reloadTerminalSettingsStore } = await import("@/state/terminalSettingsStore")
    reloadTerminalSettingsStore()
    useTerminalSettingsStore.getState().update({ copyOnSelect: false })
    useTerminalSettingsStore.getState().update({ fontSize: 18 })
    reloadTerminalSettingsStore()
    expect(useTerminalSettingsStore.getState().copyOnSelect).toBe(false)
  })
  it("defaults missing and unknown fonts while retaining legacy font size", () => {
    for (const fontFamily of [undefined, "unknown", null, 2]) {
      writeJsonSetting(TERMINAL_SETTINGS_STORAGE_KEY, { fontSize: 18, fontFamily })
      expect(loadTerminalSettings()).toMatchObject({ fontSize: 18, fontFamily: "jetbrains" })
    }
  })

  it("persists each supported font", async () => {
    const { useTerminalSettingsStore, reloadTerminalSettingsStore } = await import("@/state/terminalSettingsStore")
    reloadTerminalSettingsStore()
    for (const fontFamily of ["jetbrains", "system", "menlo", "cascadia", "consolas"] as const) {
      useTerminalSettingsStore.getState().update({ fontFamily })
      reloadTerminalSettingsStore()
      expect(useTerminalSettingsStore.getState()).toMatchObject({ fontFamily })
    }
  })
})

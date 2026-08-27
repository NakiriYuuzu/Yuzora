import { beforeEach, describe, expect, it } from "vitest"

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
  localStorage.clear()
})

describe("appearance settings", () => {
  it("沒有持久化值時回傳預設 auto 與 lime", () => {
    expect(loadAppearanceSettings()).toEqual({ theme: "auto", accent: "lime" })
  })

  it("壞 JSON 時回傳預設 auto 與 lime", () => {
    localStorage.setItem(APPEARANCE_SETTINGS_STORAGE_KEY, "{not json")
    expect(loadAppearanceSettings()).toEqual({ theme: "auto", accent: "lime" })
  })

  it("非法 theme 或 accent 值時分別回傳預設值", () => {
    localStorage.setItem(
      APPEARANCE_SETTINGS_STORAGE_KEY,
      JSON.stringify({ theme: "neon", accent: "infrared" })
    )
    expect(loadAppearanceSettings()).toEqual({ theme: "auto", accent: "lime" })
    localStorage.setItem(
      APPEARANCE_SETTINGS_STORAGE_KEY,
      JSON.stringify({ theme: 42, accent: 42 })
    )
    expect(loadAppearanceSettings()).toEqual({ theme: "auto", accent: "lime" })
  })

  it("不把 Object prototype inherited keys 當成合法 accent", () => {
    for (const accent of ["constructor", "toString"]) {
      localStorage.setItem(
        APPEARANCE_SETTINGS_STORAGE_KEY,
        JSON.stringify({ theme: "dark", accent }),
      )
      expect(loadAppearanceSettings()).toEqual({ theme: "dark", accent: "lime" })
    }
  })

  it("save→load 往返保留合法 theme 與 accent", () => {
    for (const theme of ["light", "dark", "auto"] as const) {
      saveAppearanceSettings({ theme, accent: "violet" })
      expect(loadAppearanceSettings()).toEqual({ theme, accent: "violet" })
    }
  })
})

describe("terminal settings", () => {
  it("uses a structured system-default profile and cursor IME anchor by default", () => {
    expect(loadTerminalSettings()).toMatchObject({
      defaultProfile: {
        id: "system",
        name: "System default",
        shell: "",
        args: [],
        kind: "system",
        cwdStrategy: "native",
      },
      customProfile: {
        id: "custom",
        shell: "",
        args: [],
        kind: "custom",
        cwdStrategy: "native",
      },
      imeAnchorMode: "cursor",
    })
  })

  it("migrates the legacy shell path and whitespace args into a custom profile", () => {
    localStorage.setItem(
      TERMINAL_SETTINGS_STORAGE_KEY,
      JSON.stringify({
        shellPath: " C:\\Program Files\\PowerShell\\7\\pwsh.exe ",
        shellArgs: "-NoLogo -NoProfile",
      }),
    )

    expect(loadTerminalSettings()).toMatchObject({
      defaultProfile: {
        id: "custom",
        shell: "C:\\Program Files\\PowerShell\\7\\pwsh.exe",
        args: ["-NoLogo", "-NoProfile"],
        kind: "custom",
        cwdStrategy: "native",
      },
      customProfile: {
        id: "custom",
        shell: "C:\\Program Files\\PowerShell\\7\\pwsh.exe",
        args: ["-NoLogo", "-NoProfile"],
        kind: "custom",
        cwdStrategy: "native",
      },
      imeAnchorMode: "cursor",
    })
  })

  it("preserves structured argv entries containing spaces and validates the IME anchor", () => {
    writeJsonSetting(TERMINAL_SETTINGS_STORAGE_KEY, {
      defaultProfile: {
        id: "powershell-7",
        name: "PowerShell 7",
        shell: "pwsh.exe",
        args: ["-NoExit", "-Command", "Write-Output 'hello world'"],
        kind: "powershell",
        cwdStrategy: "native",
      },
      customProfile: {
        id: "custom",
        name: "Custom",
        shell: "",
        args: [],
        kind: "custom",
        cwdStrategy: "native",
      },
      imeAnchorMode: "tui",
    })

    expect(loadTerminalSettings()).toMatchObject({
      defaultProfile: {
        args: ["-NoExit", "-Command", "Write-Output 'hello world'"],
      },
      imeAnchorMode: "tui",
    })

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

// P5：pi 雙 runtime——builtin（bundle 內 adapter）預設、community 一鍵回退。
// builtin command 來自 platform cache（非 Tauri／未 init 時為 null → 退回 community）。

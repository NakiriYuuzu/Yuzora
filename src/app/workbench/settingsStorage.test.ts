import { afterEach, beforeEach, describe, expect, it } from "vitest"

import { setCachedBuiltinPiAdapterCommandForTests } from "@/lib/platform"

import {
  AGENT_SETTINGS_STORAGE_KEY,
  APPEARANCE_SETTINGS_STORAGE_KEY,
  loadAgentSettings,
  loadAppearanceSettings,
  resolveAgentCommandRoute,
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

// P5：pi 雙 runtime——builtin（bundle 內 adapter）預設、community 一鍵回退。
// builtin command 來自 platform cache（非 Tauri／未 init 時為 null → 退回 community）。
describe("pi runtime route", () => {
  afterEach(() => {
    setCachedBuiltinPiAdapterCommandForTests(null)
  })

  it("預設 piRuntime=builtin；非法值正規化回 builtin", () => {
    expect(loadAgentSettings().piRuntime).toBe("builtin")
    writeJsonSetting(AGENT_SETTINGS_STORAGE_KEY, { piRuntime: "community" })
    expect(loadAgentSettings().piRuntime).toBe("community")
    writeJsonSetting(AGENT_SETTINGS_STORAGE_KEY, { piRuntime: "bogus" })
    expect(loadAgentSettings().piRuntime).toBe("builtin")
  })

  it("builtin cache 就緒時 pi+latest 路由到 bundle adapter，trust 維持 curated pi", () => {
    setCachedBuiltinPiAdapterCommandForTests('node "/App/Resources/adapters/yuzora-pi-acp/index.mjs"')
    const route = resolveAgentCommandRoute("pi")
    expect(route).toEqual({
      selectedPreset: "pi",
      commandMode: "latest",
      command: 'node "/App/Resources/adapters/yuzora-pi-acp/index.mjs"',
      trustedAgentId: "pi",
    })
  })

  it("cache 未就緒（null）或 piRuntime=community 時退回社群 command", () => {
    expect(resolveAgentCommandRoute("pi").command).toBe("bunx pi-acp@latest")

    setCachedBuiltinPiAdapterCommandForTests('node "/App/adapter.mjs"')
    writeJsonSetting(AGENT_SETTINGS_STORAGE_KEY, { piRuntime: "community" })
    expect(resolveAgentCommandRoute("pi").command).toBe("bunx pi-acp@latest")
  })

  it("custom mode 與其他 preset 不受 runtime 選擇影響", () => {
    setCachedBuiltinPiAdapterCommandForTests('node "/App/adapter.mjs"')
    writeJsonSetting(AGENT_SETTINGS_STORAGE_KEY, {
      presetCommands: { pi: { mode: "custom", customCommand: "my-pi --flag" } },
    })
    const route = resolveAgentCommandRoute("pi")
    expect(route.command).toBe("my-pi --flag")
    expect(route.trustedAgentId).toBeNull()
    expect(resolveAgentCommandRoute("claude").command).toBe("bunx @agentclientprotocol/claude-agent-acp@latest")
  })
})

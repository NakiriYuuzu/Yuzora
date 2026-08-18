import { describe, expect, it } from "vitest"

import {
  HERDR_ENABLED_RUNTIME_TARGETS_STORAGE_KEY,
  HERDR_NATIVE_RUNTIME_TARGET,
  HERDR_RUNTIME_TARGET_STORAGE_KEY,
  herdrRuntimeKey,
  herdrSessionKey,
  loadEnabledHerdrRuntimeTargets,
  loadHerdrRuntimeTarget,
  normalizeHerdrRuntimeTarget,
  persistEnabledHerdrRuntimeTargets,
  persistHerdrRuntimeTarget,
  sameHerdrRuntimeTarget
} from "./herdrRuntime"

describe("herdrRuntime identity", () => {
  it("keeps Native/default and WSL Ubuntu/default keys distinct", () => {
    const native = herdrSessionKey(HERDR_NATIVE_RUNTIME_TARGET, "default")
    const ubuntu = herdrSessionKey({ kind: "wsl", distro: "Ubuntu" }, "default")
    expect(native).not.toBe(ubuntu)
    expect(native).toBe("native/default")
    expect(ubuntu).toBe("wsl:Ubuntu/default")
  })

  it("escapes session and distro separators without ad-hoc caller encoding", () => {
    expect(herdrRuntimeKey({ kind: "wsl", distro: "Ubuntu Dev" })).toBe("wsl:Ubuntu%20Dev")
    expect(herdrSessionKey({ kind: "wsl", distro: "Ubuntu" }, "a/b")).toBe("wsl:Ubuntu/a%2Fb")
  })

  it("keeps delimiter, whitespace, and Unicode runtime/session keys distinct", () => {
    expect(
      herdrSessionKey({ kind: "wsl", distro: "Ubuntu:開発" }, "default: session / α")
    ).not.toBe(
      herdrSessionKey({ kind: "wsl", distro: "Ubuntu" }, "開発:default: session / α")
    )
  })

  it("persists a selected WSL target and fails closed on malformed metadata", () => {
    const values = new Map<string, string>()
    Object.defineProperty(globalThis, "localStorage", {
      configurable: true,
      value: {
        getItem: (key: string) => values.get(key) ?? null,
        setItem: (key: string, value: string) => values.set(key, value),
        removeItem: (key: string) => values.delete(key)
      }
    })
    persistHerdrRuntimeTarget({ kind: "wsl", distro: "Ubuntu 開発" })
    expect(loadHerdrRuntimeTarget()).toEqual({ kind: "wsl", distro: "Ubuntu 開発" })
    localStorage.setItem(HERDR_RUNTIME_TARGET_STORAGE_KEY, JSON.stringify({ kind: "wsl", distro: "" }))
    expect(loadHerdrRuntimeTarget()).toBe(HERDR_NATIVE_RUNTIME_TARGET)
  })

  it("persists enabled targets with Native and dedupes legacy entries", () => {
    const values = new Map<string, string>()
    Object.defineProperty(globalThis, "localStorage", {
      configurable: true,
      value: {
        getItem: (key: string) => values.get(key) ?? null,
        setItem: (key: string, value: string) => values.set(key, value)
      }
    })
    persistEnabledHerdrRuntimeTargets([
      { kind: "wsl", distro: "Ubuntu" },
      { kind: "wsl", distro: "Ubuntu" }
    ])
    expect(loadEnabledHerdrRuntimeTargets()).toEqual([
      { kind: "native" },
      { kind: "wsl", distro: "Ubuntu" }
    ])
    localStorage.setItem(HERDR_ENABLED_RUNTIME_TARGETS_STORAGE_KEY, "not-json")
    expect(loadEnabledHerdrRuntimeTargets()).toEqual([{ kind: "native" }])
  })

  it("normalizes absent persisted metadata to Native", () => {
    expect(normalizeHerdrRuntimeTarget(null)).toBe(HERDR_NATIVE_RUNTIME_TARGET)
    expect(sameHerdrRuntimeTarget(undefined, { kind: "native" })).toBe(true)
    expect(sameHerdrRuntimeTarget(undefined, { kind: "wsl", distro: "Ubuntu" })).toBe(false)
  })
})

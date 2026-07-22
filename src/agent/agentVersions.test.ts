import { beforeEach, describe, expect, it } from "vitest"

import type { AgentLatestVersion } from "@/lib/ipc"
import {
  AGENT_VERSION_STORAGE_KEY,
  agentUpdateFor,
  indexLatestAgentVersions,
  loadAgentVersions,
  rememberAgentVersion,
} from "./agentVersions"

function installLocalStorage(): void {
  const store = new Map<string, string>()
  Object.defineProperty(globalThis, "localStorage", {
    value: {
      getItem: (key: string) => store.get(key) ?? null,
      setItem: (key: string, value: string) => void store.set(key, String(value)),
      removeItem: (key: string) => void store.delete(key),
      clear: () => store.clear(),
      key: (index: number) => [...store.keys()][index] ?? null,
      get length() { return store.size },
    },
    configurable: true,
    writable: true,
  })
}

describe("agentVersions", () => {
  beforeEach(() => {
    installLocalStorage()
    localStorage.clear()
  })

  it("persists normalized versions without retaining arbitrary keys", () => {
    localStorage.setItem(AGENT_VERSION_STORAGE_KEY, JSON.stringify({ pi: " v0.0.30 ", unknown: "9" }))
    expect(loadAgentVersions()).toEqual({ pi: "0.0.30" })

    rememberAgentVersion("pi", "v0.0.31")
    expect(loadAgentVersions()).toEqual({ pi: "0.0.31" })
  })

  it("indexes only valid latest-version rows", () => {
    const rows: AgentLatestVersion[] = [
      { agentId: "pi", version: "v0.0.32" },
      { agentId: "claude", version: " " },
    ]
    expect(indexLatestAgentVersions(rows)).toEqual({ pi: "0.0.32" })
  })

  it("reports an update only when both normalized versions exist and differ", () => {
    expect(agentUpdateFor("pi", { pi: "0.0.31" }, { pi: "0.0.32" })).toEqual({
      currentVersion: "0.0.31",
      latestVersion: "0.0.32",
    })
    expect(agentUpdateFor("pi", { pi: "v0.0.32" }, { pi: "0.0.32" })).toBeNull()
    expect(agentUpdateFor("pi", {}, { pi: "0.0.32" })).toBeNull()
    expect(agentUpdateFor("pi", { pi: "0.0.31" }, {})).toBeNull()
  })
})

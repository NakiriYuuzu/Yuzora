import { Server } from "lucide-react"
import { describe, expect, it } from "vitest"

import { DEFAULT_MODE, MODES, normalizeWorkbenchMode } from "./modes"

describe("modes", () => {
  it("puts ADE first and Files second, with ADE as default", () => {
    expect(MODES.map((m) => m.id)).toEqual(["ade", "files", "git", "database", "ssh"])
    expect(DEFAULT_MODE).toBe("ade")
  })

  it("uses the server icon for the SSH/SFTP mode", () => {
    expect(MODES.find((mode) => mode.id === "ssh")?.icon).toBe(Server)
  })

  it("normalizes legacy agent/agentzone ids to ade", () => {
    expect(normalizeWorkbenchMode("agent")).toBe("ade")
    expect(normalizeWorkbenchMode("agentzone")).toBe("ade")
    expect(normalizeWorkbenchMode("AgentZone")).toBe("ade")
    expect(normalizeWorkbenchMode("files")).toBe("files")
    expect(normalizeWorkbenchMode("nope")).toBe("ade")
  })
})

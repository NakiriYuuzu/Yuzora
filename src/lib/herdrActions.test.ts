import { describe, expect, it } from "vitest"
import type { HerdrCapabilities, HerdrSessionRuntime, HerdrSnapshot } from "@/lib/herdrTypes"
import { AGENT_NAME_PATTERN, agentTaskForPane, herdrActionAvailability, herdrTaskAvailability, suggestAgentName } from "./herdrActions"

const runtime = (patch: Partial<HerdrSessionRuntime> = {}, methods = ["agent.start", "pane.move"], server = { running: true, compatible: true, version: "0.9.1" }): HerdrSessionRuntime => ({
  connectionState: "ready", errorMessage: null, worktreeInventory: null,
  capabilities: { binaryVersion: "0.9.1", server, api: { snapshot: true, methods } } as unknown as HerdrCapabilities,
  snapshot: { agents: [], spaces: [{ id: "w1" }], terminals: [{ terminalId: "t", paneId: "p1" }], tabs: [] } as unknown as HerdrSnapshot,
  ...patch,
})

describe("herdrActionAvailability", () => {
  it("reports the first failing gate with a reason", () => {
    expect(herdrActionAvailability(undefined, "agent.start")).toMatchObject({ ok: false, reason: expect.stringContaining("not connected") })
    expect(herdrActionAvailability(undefined, "agent.start", true)).toMatchObject({ ok: false, reason: expect.stringContaining("still starting") })
    expect(herdrActionAvailability(runtime({ connectionState: "connecting" }), "agent.start")).toMatchObject({ ok: false, reason: expect.stringContaining("still starting") })
    expect(herdrActionAvailability(runtime({}, [], { running: false, compatible: true, version: "0.9.1" }), "agent.start")).toMatchObject({ ok: false, reason: expect.stringContaining("server is not running") })
    expect(herdrActionAvailability(runtime(), "plugin.list")).toMatchObject({ ok: false, reason: expect.stringContaining("(current 0.9.1)") })
    expect(herdrActionAvailability(runtime(), "agent.start")).toEqual({ ok: true })
  })
})

describe("herdrTaskAvailability", () => {
  it("explains missing Space, Pane and Agent and never gates Session management", () => {
    expect(herdrTaskAvailability("sessions", undefined)).toEqual({ ok: true })
    const empty = runtime({ snapshot: { agents: [], spaces: [], terminals: [], tabs: [] } as unknown as HerdrSnapshot }, ["worktree.create", "agent.start", "agent.prompt", "pane.move"])
    expect(herdrTaskAvailability("worktree", empty)).toMatchObject({ ok: false, reason: expect.stringContaining("no Space") })
    expect(herdrTaskAvailability("startAgent", empty)).toMatchObject({ ok: false, reason: expect.stringContaining("no Pane") })
    expect(herdrTaskAvailability("movePane", empty)).toMatchObject({ ok: false, reason: expect.stringContaining("no Pane") })
    expect(herdrTaskAvailability("messageAgent", runtime({}, ["agent.prompt"]))).toMatchObject({ ok: false, reason: expect.stringContaining("running an agent") })
    expect(herdrTaskAvailability("startAgent", runtime())).toEqual({ ok: true })
  })
})

describe("agent helpers", () => {
  const snapshot = { agents: [{ id: "a", name: "codex", paneId: "p2", status: "idle", workspaceId: "w1" }, { id: "b", name: "codex-2", paneId: "p3", status: "idle", workspaceId: "w1" }] } as unknown as HerdrSnapshot
  it("routes a pane to message or start by agent presence", () => {
    expect(agentTaskForPane(snapshot, "p2")).toBe("messageAgent")
    expect(agentTaskForPane(snapshot, "p1")).toBe("startAgent")
    expect(agentTaskForPane(snapshot, undefined)).toBe("startAgent")
    expect(agentTaskForPane(null, "p1")).toBe("startAgent")
  })
  it("suggests a unique name that satisfies the agent name pattern", () => {
    expect(suggestAgentName("claude", snapshot)).toBe("claude")
    expect(suggestAgentName("codex", snapshot)).toBe("codex-3")
    for (const kind of ["claude", "codex", "pi", "gemini", "cursor", "opencode", "qodercli", "mastracode"]) {
      expect(suggestAgentName(kind, snapshot)).toMatch(AGENT_NAME_PATTERN)
      expect(suggestAgentName(kind, { agents: [{ name: kind }] } as unknown as HerdrSnapshot)).toMatch(AGENT_NAME_PATTERN)
    }
  })
})

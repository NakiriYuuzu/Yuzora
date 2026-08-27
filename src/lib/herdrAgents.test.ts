import { describe, expect, it } from "vitest"

import { sortHerdrAgentsByUrgency } from "./herdrAgents"
import type { HerdrAgentInfo, HerdrAgentStatus } from "./herdrTypes"

function agent(id: string, status: HerdrAgentStatus, title = id): HerdrAgentInfo {
  return {
    id,
    name: id,
    title,
    status,
    workspaceId: "w1"
  }
}

describe("sortHerdrAgentsByUrgency", () => {
  it("orders blocked, done, working, unknown, then idle without mutating input", () => {
    const input = [
      agent("idle", "idle"),
      agent("working", "working"),
      agent("blocked", "blocked"),
      agent("unknown", "unknown"),
      agent("done", "done")
    ]

    expect(sortHerdrAgentsByUrgency(input).map((item) => item.id)).toEqual([
      "blocked",
      "done",
      "working",
      "unknown",
      "idle"
    ])
    expect(input[0]?.id).toBe("idle")
  })

  it("uses a deterministic label tie-break inside one status", () => {
    const input = [
      agent("second", "working", "Agent 10"),
      agent("first", "working", "Agent 2")
    ]
    expect(sortHerdrAgentsByUrgency(input).map((item) => item.id)).toEqual([
      "first",
      "second"
    ])
  })
})

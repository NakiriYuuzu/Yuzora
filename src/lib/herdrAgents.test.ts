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

  it("preserves locale-aware numeric labels, title fallback and ID tie-breaks", () => {
    const labels = [
      "", " ", "Agent 2", "agent 02", "AGENT 10", "Agent 0002",
      "a", "A", "á", "a\u0301", "ä", "Å", "ß", "ss", "İ", "I", "ı", "i",
      "中文", "臺灣", "台湾", "が", "か\u3099", "한글", "مرحبا", "שלום", "😀", "🧑‍💻"
    ]
    const input: HerdrAgentInfo[] = labels.flatMap((label, index) => [
      agent(`${index}-b`, "working", label),
      { id: `${index}-a`, name: label, status: "working", workspaceId: "w1" }
    ])
    input.push(agent("id-2", "working", "Agent 2"), agent("id-10", "working", "agent 02"))
    const expected = [...input].sort((left, right) =>
      (left.title ?? left.name).localeCompare(right.title ?? right.name, undefined, {
        numeric: true,
        sensitivity: "base"
      }) || left.id.localeCompare(right.id)
    )

    expect(sortHerdrAgentsByUrgency(input)).toEqual(expected)
  })
})

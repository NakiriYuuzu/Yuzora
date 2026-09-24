import { describe, expect, it } from "vitest"
import type { HerdrAttentionItem } from "./herdrTypes"
import { newHerdrAttention } from "./herdrNotifications"
const item = (key: string, kind: HerdrAttentionItem["kind"] = "done"): HerdrAttentionItem => ({ key, sessionName: key, paneId: "w1:p1", kind, agentStatus: kind, seen: false, updatedAt: 1 })
const map = (...items: HerdrAttentionItem[]) => new Map(items.map(i => [i.key, i]))

describe("HERDR notification transitions", () => {
  it("deduplicates refreshes and ignores already-seen and unknown attention", () => {
    expect(newHerdrAttention(map(item("a")), map({ ...item("a"), updatedAt: 2 }, { ...item("b"), seen: true }, item("c", "unknown")))).toEqual([])
  })
  it("notifies a new state or a new turn after the attention was cleared", () => {
    expect(newHerdrAttention(map(item("a", "blocked")), map(item("a")))).toEqual([item("a")])
    expect(newHerdrAttention(map(), map(item("a")))).toEqual([item("a")])
  })
  it("keeps identical pane names on separate hosts and Sessions independent", () => {
    const a = item('["host-a","default","w1:p1"]'), b = item('["host-b","default","w1:p1"]')
    expect(newHerdrAttention(map(a), map(a, b))).toEqual([b])
  })
})

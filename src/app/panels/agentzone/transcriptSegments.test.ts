import { describe, expect, it } from "vitest"

import type { BlockEntry, MsgEntry, TranscriptEntry } from "@/agent/acpTypes"

import { segmentTranscript } from "./transcriptSegments"

let seq = 0
function msg(who: MsgEntry["who"], text: string): MsgEntry {
  return { id: `m${++seq}`, who, text, streaming: false }
}
function block(kind: BlockEntry["kind"], text: string = kind): BlockEntry {
  return { id: `b${++seq}`, kind, text }
}

describe("segmentTranscript", () => {
  it("groups consecutive tool and thought entries into one activity segment", () => {
    const thought = block("thought")
    const tool1 = block("tool", "Read a.ts")
    const tool2 = block("tool", "Edit a.ts")
    const segments = segmentTranscript([msg("you", "hi"), thought, tool1, tool2])

    expect(segments).toHaveLength(2)
    expect(segments[1]).toEqual({ type: "activity", id: thought.id, entries: [thought, tool1, tool2] })
  })

  it("keeps a single tool entry as an activity segment", () => {
    const tool = block("tool")
    const segments = segmentTranscript([tool])
    expect(segments).toEqual([{ type: "activity", id: tool.id, entries: [tool] }])
  })

  it.each(["perm", "plan", "diff", "error", "notice"] as const)(
    "breaks the chain on a %s block",
    (kind) => {
      const tool1 = block("tool")
      const breaker = block(kind)
      const tool2 = block("tool")
      const segments = segmentTranscript([tool1, breaker, tool2])

      expect(segments).toEqual([
        { type: "activity", id: tool1.id, entries: [tool1] },
        { type: "entry", entry: breaker },
        { type: "activity", id: tool2.id, entries: [tool2] },
      ])
    }
  )

  it("breaks the chain on user and agent messages", () => {
    const tool1 = block("tool")
    const reply = msg("agent", "done")
    const tool2 = block("tool")
    const segments = segmentTranscript([tool1, reply, tool2])

    expect(segments).toEqual([
      { type: "activity", id: tool1.id, entries: [tool1] },
      { type: "entry", entry: reply },
      { type: "activity", id: tool2.id, entries: [tool2] },
    ])
  })

  it("keeps the activity segment id stable while the chain grows at the tail", () => {
    const tool1 = block("tool")
    const entries: TranscriptEntry[] = [tool1]
    const before = segmentTranscript(entries)
    const after = segmentTranscript([...entries, block("tool"), block("thought")])

    expect(before[0].type).toBe("activity")
    expect(after[0].type).toBe("activity")
    expect((after[0] as { id: string }).id).toBe((before[0] as { id: string }).id)
  })

  it("returns plain entry segments for a transcript without activity", () => {
    const a = msg("you", "hi")
    const b = block("plan")
    expect(segmentTranscript([a, b])).toEqual([
      { type: "entry", entry: a },
      { type: "entry", entry: b },
    ])
  })
})

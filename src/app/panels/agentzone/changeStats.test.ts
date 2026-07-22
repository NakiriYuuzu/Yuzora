import { describe, expect, it } from "vitest"

import type { TranscriptEntry } from "@/agent/acpTypes"

import { collectChangeStats } from "./changeStats"

function tool(id: string, meta: Record<string, unknown>): TranscriptEntry {
    return { id, kind: "tool", text: "edit", meta: JSON.stringify({ toolCallId: id, ...meta }) }
}

describe("collectChangeStats", () => {
    it("returns null when no tool entry carries diff stats", () => {
        expect(collectChangeStats([])).toBeNull()
        expect(
            collectChangeStats([
                { id: "m1", who: "agent", text: "hi", streaming: false },
                tool("t1", { kind: "read" }),
                { id: "t2", kind: "tool", text: "x", meta: "not-json" },
            ])
        ).toBeNull()
    })

    it("aggregates per-path line counts across entries and sums totals", () => {
        const stats = collectChangeStats([
            tool("t1", { diffs: [{ path: "src/a.ts", added: 5, removed: 2 }] }),
            tool("t2", {
                diffs: [
                    { path: "src/b.ts", added: 1, removed: 0 },
                    { path: "src/a.ts", added: 2, removed: 1 },
                ],
            }),
        ])
        expect(stats).toEqual({
            files: [
                { path: "src/a.ts", added: 7, removed: 3 },
                { path: "src/b.ts", added: 1, removed: 0 },
            ],
            added: 8,
            removed: 3,
        })
    })

    it("skips malformed diff items but keeps valid ones", () => {
        const stats = collectChangeStats([
            tool("t1", {
                diffs: [
                    { path: "src/a.ts", added: 1, removed: 0 },
                    { path: 42, added: "x" },
                    null,
                ],
            }),
        ])
        expect(stats).toEqual({
            files: [{ path: "src/a.ts", added: 1, removed: 0 }],
            added: 1,
            removed: 0,
        })
    })
})

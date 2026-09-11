import { describe, expect, it } from "vitest"

import { computeGraphLayout } from "./graphLayout"
import type { GraphInputCommit, GraphSegment } from "./graphLayout"

// Helper: commit list in git-log order (newest first).
const c = (hash: string, parents: string[] = []): GraphInputCommit => ({ hash, parents })

// Segments are order-independent; sort for stable comparison.
const sortSegs = (segs: GraphSegment[]): GraphSegment[] =>
    [...segs].sort((a, b) => a.fromLane - b.fromLane || a.toLane - b.toLane)

describe("computeGraphLayout", () => {
    it("connects a merge to a parent already carried by another active lane", () => {
        const { rows } = computeGraphLayout([
            c("other-tip", ["P"]), c("merge", ["Q", "P"]), c("Q", ["base"]), c("P", ["base"]), c("base")
        ])
        expect(rows[1].segments).toContainEqual({ fromLane: 1, toLane: 0, colorIdx: 0, phase: "outgoing" })
        expect(rows[1].segments).toContainEqual({ fromLane: 0, toLane: 0, colorIdx: 0, phase: "through" })
        expect(rows[1].segments.filter((segment) => segment.phase === "outgoing").map((segment) => segment.toLane)).toEqual([1, 0])
    })

    it("terminates disconnected roots at their node while unrelated lanes pass through", () => {
        const { rows } = computeGraphLayout([c("tip", ["base"]), c("isolated"), c("base")])
        expect(rows[0].segments.every((segment) => segment.phase === "outgoing")).toBe(true)
        expect(rows[1].lane).toBe(1)
        expect(rows[1].segments).toEqual([{ fromLane: 0, toLane: 0, colorIdx: 0, phase: "through" }])
        expect(rows[2].segments).toEqual([{ fromLane: 0, toLane: 0, colorIdx: 0, phase: "incoming" }])
    })
    it("1. linear history → all lane 0, straight segments", () => {
        const { rows, laneCount } = computeGraphLayout([
            c("D", ["C"]),
            c("C", ["B"]),
            c("B", ["A"]),
            c("A", [])
        ])
        expect(laneCount).toBe(1)
        for (const r of rows) {
            expect(r.lane).toBe(0)
            expect(r.colorIdx).toBe(0)
            expect(r.isMerge).toBe(false)
        }
        expect(rows[0].segments).toEqual([{ fromLane: 0, toLane: 0, colorIdx: 0, phase: "outgoing" }])
        for (const row of rows.slice(1, 3)) expect(row.segments).toEqual([
            { fromLane: 0, toLane: 0, colorIdx: 0, phase: "incoming" },
            { fromLane: 0, toLane: 0, colorIdx: 0, phase: "outgoing" }
        ])
        expect(rows[3].segments).toEqual([{ fromLane: 0, toLane: 0, colorIdx: 0, phase: "incoming" }])
    })

    it("2. single branch + merge → two lanes, branch-out and merge-in segments", () => {
        // M(parents B,C) — B(parent A) — C(parent A) — A(root); log order M,B,C,A.
        const { rows, laneCount } = computeGraphLayout([
            c("M", ["B", "C"]),
            c("B", ["A"]),
            c("C", ["A"]),
            c("A", [])
        ])
        expect(laneCount).toBe(2)
        const byHash = Object.fromEntries(rows.map((r) => [r.hash, r]))

        // M: merge on lane 0, opens a second lane for parent C → node fans out to
        // lane 0 (first parent B) and lane 1 (second parent C), both from node.
        expect(byHash.M.lane).toBe(0)
        expect(byHash.M.isMerge).toBe(true)
        expect(sortSegs(byHash.M.segments)).toEqual([
            { fromLane: 0, toLane: 0, colorIdx: 0, phase: "outgoing" },
            { fromLane: 0, toLane: 1, colorIdx: 1, phase: "outgoing" }
        ])

        // B on lane 0 continues to A; lane 1 (C branch) passes straight through.
        expect(byHash.B.lane).toBe(0)
        expect(sortSegs(byHash.B.segments)).toEqual([
            { fromLane: 0, toLane: 0, colorIdx: 0, phase: "incoming" },
            { fromLane: 0, toLane: 0, colorIdx: 0, phase: "outgoing" },
            { fromLane: 1, toLane: 1, colorIdx: 1, phase: "through" }
        ])

        // C on lane 1 also targets A. Both lanes now wait for A, so they run
        // parallel straight down; the actual convergence happens at row A below,
        // not here (standard lane behaviour — the merge-in curve lands on A).
        expect(byHash.C.lane).toBe(1)
        expect(sortSegs(byHash.C.segments)).toEqual([
            { fromLane: 0, toLane: 0, colorIdx: 0, phase: "through" },
            { fromLane: 1, toLane: 1, colorIdx: 1, phase: "incoming" },
            { fromLane: 1, toLane: 1, colorIdx: 1, phase: "outgoing" }
        ])

        // A is the root: both lanes converge onto its node lane 0 (merge-in).
        expect(byHash.A.lane).toBe(0)
        expect(sortSegs(byHash.A.segments)).toEqual([
            { fromLane: 0, toLane: 0, colorIdx: 0, phase: "incoming" },
            { fromLane: 1, toLane: 0, colorIdx: 1, phase: "incoming" }
        ])
    })

    it("3. two parallel branches → stable lane 0/1, distinct colours", () => {
        // Independent tips X and Y sharing base A; neither has a waiting lane so
        // each opens a fresh lane with its own colour.
        const { rows, laneCount } = computeGraphLayout([
            c("X", ["A"]),
            c("Y", ["A"]),
            c("A", [])
        ])
        expect(laneCount).toBe(2)
        const byHash = Object.fromEntries(rows.map((r) => [r.hash, r]))

        expect(byHash.X.lane).toBe(0)
        expect(byHash.Y.lane).toBe(1)
        expect(byHash.X.colorIdx).not.toBe(byHash.Y.colorIdx)

        // Y row: X's lane 0 passes straight through toward A, Y opens lane 1.
        expect(sortSegs(byHash.Y.segments)).toEqual([
            { fromLane: 0, toLane: 0, colorIdx: byHash.X.colorIdx, phase: "through" },
            { fromLane: 1, toLane: 1, colorIdx: byHash.Y.colorIdx, phase: "outgoing" }
        ])
        // A converges both lanes onto lane 0.
        expect(byHash.A.lane).toBe(0)
        expect(sortSegs(byHash.A.segments)).toEqual([
            { fromLane: 0, toLane: 0, colorIdx: byHash.X.colorIdx, phase: "incoming" },
            { fromLane: 1, toLane: 0, colorIdx: byHash.Y.colorIdx, phase: "incoming" }
        ])
    })

    it("4. root commit (no parents) → lane terminates, no dangling segments", () => {
        const { rows, laneCount } = computeGraphLayout([c("A", [])])
        expect(laneCount).toBe(1)
        expect(rows[0].lane).toBe(0)
        expect(rows[0].isMerge).toBe(false)
        // A lone root has nobody flowing into it and no parent flowing out.
        expect(rows[0].segments).toEqual([])
    })

    it("5. lane recycling → a new branch reuses a freed lane slot", () => {
        // Newest-first: F merges D+E; the E branch closes; later independent tip
        // G should recycle the freed lane rather than widening the graph.
        //   F(D,E) · D(C) · E(C) · C(B) · G(B) · B(A) · A()
        const { rows } = computeGraphLayout([
            c("F", ["D", "E"]),
            c("D", ["C"]),
            c("E", ["C"]),
            c("C", ["B"]),
            c("G", ["B"]),
            c("B", ["A"]),
            c("A", [])
        ])
        const byHash = Object.fromEntries(rows.map((r) => [r.hash, r]))
        // After the D/E branches converge at C (back to one lane), G opens a lane
        // and reuses the freed slot 1 instead of growing to lane 2.
        expect(byHash.G.lane).toBe(1)
        const maxLaneSeen = Math.max(
            ...rows.flatMap((r) => [
                r.lane,
                ...r.segments.flatMap((s) => [s.fromLane, s.toLane])
            ])
        )
        expect(maxLaneSeen).toBe(1)
    })

    it("6. out-of-order guard → parent before child does not crash", () => {
        // Pathological: parent A emitted before child B. git log never does this,
        // but the layout must not throw or loop forever.
        expect(() => computeGraphLayout([c("A", []), c("B", ["A"])])).not.toThrow()
        const { rows } = computeGraphLayout([c("A", []), c("B", ["A"])])
        expect(rows).toHaveLength(2)
        const b = rows.find((r) => r.hash === "B")!
        expect(b.lane).toBeGreaterThanOrEqual(0)
    })

    it.each([16, 32])("keeps all %i parallel branch lanes distinct through their parent rows", (count) => {
        const commits = [
            ...Array.from({ length: count }, (_, i) => c(`tip${i}`, [`parent${i}`])),
            ...Array.from({ length: count }, (_, i) => c(`parent${i}`))
        ]
        const { rows, laneCount } = computeGraphLayout(commits)
        expect(laneCount).toBe(count)
        for (let i = 0; i < count; i++) {
            expect(rows[i].lane).toBe(i)
            expect(rows[count + i].lane).toBe(i)
            expect(rows[count + i].colorIdx).toBe(rows[i].colorIdx)
            expect(rows[count + i].segments).toContainEqual({ fromLane: i, toLane: i, colorIdx: rows[i].colorIdx, phase: "incoming" })
        }
        expect(rows[count - 1].segments.map((segment) => segment.toLane)).toEqual(Array.from({ length: count }, (_, i) => i))
    })

    it("fans out a 32-parent octopus merge and converges every branch at its shared base", () => {
        const parents = Array.from({ length: 32 }, (_, i) => `parent${i}`)
        const { rows, laneCount } = computeGraphLayout([
            c("merge", parents), ...parents.map((hash) => c(hash, ["base"])), c("base")
        ])
        expect(laneCount).toBe(32)
        expect(rows[0].isMerge).toBe(true)
        expect(rows[0].segments).toEqual(parents.map((_, i) => ({ fromLane: 0, toLane: i, colorIdx: i, phase: "outgoing" })))
        expect(rows.at(-1)!.segments).toEqual(parents.map((_, i) => ({ fromLane: i, toLane: 0, colorIdx: i, phase: "incoming" })))
    })

    it("keeps crossing branch joins distinct and recycles the converged lane", () => {
        const { rows, laneCount } = computeGraphLayout([
            c("tip-left", ["left"]), c("tip-right", ["right"]),
            c("cross-left", ["right"]), c("cross-right", ["left"]),
            c("left", ["base"]), c("new-tip", ["base"]), c("right", ["base"]), c("base")
        ])
        const byHash = new Map(rows.map((row) => [row.hash, row]))
        expect(laneCount).toBe(4)
        expect(byHash.get("left")!.segments).toContainEqual({ fromLane: 3, toLane: 0, colorIdx: 3, phase: "incoming" })
        expect(byHash.get("new-tip")!.lane).toBe(3)
        expect(byHash.get("right")!.segments).toContainEqual({ fromLane: 2, toLane: 1, colorIdx: 2, phase: "incoming" })
        expect(byHash.get("base")!.segments.map(({ fromLane, toLane }) => ({ fromLane, toLane }))).toEqual([
            { fromLane: 0, toLane: 0 }, { fromLane: 1, toLane: 0 }, { fromLane: 3, toLane: 0 }
        ])
    })

    it("preserves every existing row when older history pages are appended", () => {
        const tips = Array.from({ length: 32 }, (_, i) => c(`tip${i}`, [`parent${i}`]))
        const older = Array.from({ length: 32 }, (_, i) => c(`parent${i}`, ["base"]))
        const firstPage = computeGraphLayout(tips)
        const nextPage = computeGraphLayout([...tips, ...older, c("base"), c("unrelated")])
        expect(firstPage.laneCount).toBe(32)
        expect(nextPage.rows.slice(0, tips.length)).toEqual(firstPage.rows)
        expect(nextPage.rows.at(-1)!.lane).toBe(0)
        expect(nextPage.laneCount).toBe(32)
    })
})

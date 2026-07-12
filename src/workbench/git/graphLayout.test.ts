import { describe, expect, it } from "vitest"

import { computeGraphLayout, MAX_LANES } from "./graphLayout"
import type { GraphInputCommit, GraphSegment } from "./graphLayout"

// Helper: commit list in git-log order (newest first).
const c = (hash: string, parents: string[] = []): GraphInputCommit => ({ hash, parents })

// Segments are order-independent; sort for stable comparison.
const sortSegs = (segs: GraphSegment[]): GraphSegment[] =>
    [...segs].sort((a, b) => a.fromLane - b.fromLane || a.toLane - b.toLane)

describe("computeGraphLayout", () => {
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
        // Each non-root row carries one straight (from==to) line down. The root
        // still receives the line arriving from above (its own node terminus).
        expect(rows[0].segments).toEqual([{ fromLane: 0, toLane: 0, colorIdx: 0 }]) // D→C
        expect(rows[1].segments).toEqual([{ fromLane: 0, toLane: 0, colorIdx: 0 }]) // C→B
        expect(rows[2].segments).toEqual([{ fromLane: 0, toLane: 0, colorIdx: 0 }]) // B→A
        expect(rows[3].segments).toEqual([{ fromLane: 0, toLane: 0, colorIdx: 0 }]) // into A
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
            { fromLane: 0, toLane: 0, colorIdx: 0 },
            { fromLane: 0, toLane: 1, colorIdx: 1 }
        ])

        // B on lane 0 continues to A; lane 1 (C branch) passes straight through.
        expect(byHash.B.lane).toBe(0)
        expect(sortSegs(byHash.B.segments)).toEqual([
            { fromLane: 0, toLane: 0, colorIdx: 0 },
            { fromLane: 1, toLane: 1, colorIdx: 1 }
        ])

        // C on lane 1 also targets A. Both lanes now wait for A, so they run
        // parallel straight down; the actual convergence happens at row A below,
        // not here (standard lane behaviour — the merge-in curve lands on A).
        expect(byHash.C.lane).toBe(1)
        expect(sortSegs(byHash.C.segments)).toEqual([
            { fromLane: 0, toLane: 0, colorIdx: 0 },
            { fromLane: 1, toLane: 1, colorIdx: 1 }
        ])

        // A is the root: both lanes converge onto its node lane 0 (merge-in).
        expect(byHash.A.lane).toBe(0)
        expect(sortSegs(byHash.A.segments)).toEqual([
            { fromLane: 0, toLane: 0, colorIdx: 0 },
            { fromLane: 1, toLane: 0, colorIdx: 1 }
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
            { fromLane: 0, toLane: 0, colorIdx: byHash.X.colorIdx },
            { fromLane: 1, toLane: 1, colorIdx: byHash.Y.colorIdx }
        ])
        // A converges both lanes onto lane 0.
        expect(byHash.A.lane).toBe(0)
        expect(sortSegs(byHash.A.segments)).toEqual([
            { fromLane: 0, toLane: 0, colorIdx: byHash.X.colorIdx },
            { fromLane: 1, toLane: 0, colorIdx: byHash.Y.colorIdx }
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

    it("caps lane width at MAX_LANES without crashing", () => {
        // Many independent tips would each open a lane; OUTPUT indices clamp to
        // the last lane instead of growing unbounded (tracking is unbounded).
        const many: GraphInputCommit[] = []
        for (let i = 0; i < MAX_LANES + 5; i++) many.push(c(`tip${i}`, [`base${i}`]))
        const { rows, laneCount } = computeGraphLayout(many, MAX_LANES)
        expect(laneCount).toBeLessThanOrEqual(MAX_LANES)
        for (const r of rows) expect(r.lane).toBeLessThan(MAX_LANES)
    })

    it("keeps overflow-lane topology: 13+ concurrent tips do not corrupt tracked lanes", () => {
        // Regression: the old cap OVERWROTE the last slot's waiting parent when
        // tip 13 arrived, so that branch's line died mid-graph and its parent
        // re-materialised as a false new tip (fresh colour, disconnected). With
        // --all feeding the graph this shape is everyday, not pathological.
        // Tracking is now unbounded; every parent must keep its tip's colour.
        const n = MAX_LANES + 3
        const many: GraphInputCommit[] = []
        for (let i = 0; i < n; i++) many.push(c(`t${i}`, [`p${i}`]))
        for (let i = 0; i < n; i++) many.push(c(`p${i}`, []))
        const { rows, laneCount } = computeGraphLayout(many, MAX_LANES)
        const byHash = new Map(rows.map((r) => [r.hash, r]))
        for (let i = 0; i < n; i++) {
            expect(byHash.get(`p${i}`)!.colorIdx).toBe(byHash.get(`t${i}`)!.colorIdx)
        }
        // Output stays clamped even though tracking exceeded the cap.
        expect(laneCount).toBeLessThanOrEqual(MAX_LANES)
        for (const r of rows) {
            expect(r.lane).toBeLessThan(MAX_LANES)
            for (const s of r.segments) {
                expect(s.fromLane).toBeLessThan(MAX_LANES)
                expect(s.toLane).toBeLessThan(MAX_LANES)
            }
        }
    })
})

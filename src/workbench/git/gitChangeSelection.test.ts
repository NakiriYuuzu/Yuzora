import { describe, expect, it } from "vitest"

import type { GitStatus } from "@/lib/types"

import {
    buildGitChangeModel,
    currentGitChanges,
    gitChangeDomId,
    gitChangeId,
    gitChangeRows,
    gitChangeVisibleOrder,
    gitChangeVisualOrder,
    sectionSelectionState,
    selectedMutationSubsets,
    toggleSectionSelection
} from "./gitChangeSelection"

function status(over: Partial<GitStatus> = {}): GitStatus {
    return {
        branch: "main",
        headOid: "0".repeat(40),
        detached: false,
        upstream: null,
        ahead: 0,
        behind: 0,
        staged: [],
        unstaged: [],
        untracked: [],
        conflicted: [],
        inProgress: null,
        ...over
    }
}

describe("gitChangeSelection visual order and mutation subsets", () => {
    it("orders visible rows conflicts → staged → unstaged → untracked", () => {
        const rows = gitChangeRows(status({
            staged: [{ path: "staged.ts", origPath: null, status: "M" }],
            unstaged: [{ path: "unstaged.ts", origPath: null, status: "M" }],
            untracked: ["new.ts"],
            conflicted: [{ path: "conf.ts", origPath: null, status: "U" }]
        }))
        expect(gitChangeVisualOrder(rows).map((row) => row.path)).toEqual([
            "conf.ts",
            "staged.ts",
            "unstaged.ts",
            "new.ts"
        ])
    })

    it("identity-caches one-pass rows, buckets, order and lookup maps", () => {
        const snapshot = status({
            staged: [{ path: "partial.ts", origPath: null, status: "M" }],
            unstaged: [
                { path: "partial.ts", origPath: null, status: "M" },
                ...Array.from({ length: 1596 }, (_, index) => ({
                    path: `src/file-${String(index).padStart(4, "0")}.ts`,
                    origPath: null,
                    status: "M"
                }))
            ]
        })
        const first = buildGitChangeModel(snapshot)
        const second = buildGitChangeModel(snapshot)

        expect(second).toBe(first)
        expect(gitChangeRows(snapshot)).toBe(first.rows)
        expect(first.visualOrder).toHaveLength(1598)
        expect(first.buckets.staged).toHaveLength(1)
        expect(first.buckets.unstaged).toHaveLength(1597)
        expect(first.rowById.get("s:partial.ts")).toBe(first.buckets.staged[0])
        expect(first.rowById.get("c:partial.ts")).toBe(first.buckets.unstaged[0])
        expect(first.indexById.get("s:partial.ts")).toBe(0)
        expect(first.indexById.get("c:partial.ts")).toBe(1)
        expect(gitChangeId(first.buckets.staged[0])).not.toBe(gitChangeId(first.buckets.unstaged[0]))
        expect(currentGitChanges([
            first.buckets.unstaged[1596],
            first.buckets.staged[0]
        ], first.rowById)).toEqual([
            first.buckets.unstaged[1596],
            first.buckets.staged[0]
        ])
    })

    it("computes section tri-state and toggle without mutating Git identities", () => {
        const rows = gitChangeRows(status({
            unstaged: [
                { path: "a.ts", origPath: null, status: "M" },
                { path: "b.ts", origPath: null, status: "M" }
            ]
        }))
        expect(sectionSelectionState(rows, [])).toBe("unchecked")
        expect(sectionSelectionState(rows, [rows[0]])).toBe("mixed")
        expect(sectionSelectionState(rows, rows)).toBe("checked")
        expect(toggleSectionSelection(rows, [rows[0]]).map((row) => row.path)).toEqual(["a.ts", "b.ts"])
        expect(toggleSectionSelection(rows, rows)).toEqual([])
    })

    it("excludes conflicts from stage/unstage/discard subsets", () => {
        const rows = gitChangeVisualOrder(gitChangeRows(status({
            staged: [{ path: "staged.ts", origPath: null, status: "M" }],
            unstaged: [{ path: "unstaged.ts", origPath: null, status: "M" }],
            untracked: ["new.ts"],
            conflicted: [{ path: "conf.ts", origPath: null, status: "U" }]
        })))
        const subsets = selectedMutationSubsets(rows)
        expect(subsets.conflicts.map((row) => row.path)).toEqual(["conf.ts"])
        expect(subsets.stageable.map((row) => row.path)).toEqual(["unstaged.ts", "new.ts"])
        expect(subsets.unstageable.map((row) => row.path)).toEqual(["staged.ts"])
        expect(subsets.discardable.map((row) => row.path)).toEqual(["unstaged.ts", "new.ts"])
    })

    it("visible order excludes collapsed section buckets", () => {
        const rows = gitChangeRows(status({
            staged: [{ path: "staged.ts", origPath: null, status: "M" }],
            unstaged: [{ path: "unstaged.ts", origPath: null, status: "M" }],
            untracked: ["new.ts"],
            conflicted: [{ path: "conf.ts", origPath: null, status: "U" }]
        }))
        expect(gitChangeVisibleOrder(rows, {
            conflicts: true,
            staged: true,
            unstaged: true,
            untracked: false
        }).map((row) => row.path)).toEqual(["conf.ts", "staged.ts", "unstaged.ts"])
    })

    it("encodes spaced paths into safe HTML ids", () => {
        expect(gitChangeDomId("local-file", { path: "my file.ts", staged: false }))
            .toBe("local-file-c-my_20file.ts")
    })

    it("encodes colliding underscore/slash and unicode paths injectively", () => {
        const slash = gitChangeDomId("local-file", { path: "a/b.ts", staged: false })
        const underscoreHex = gitChangeDomId("local-file", { path: "a_2fb.ts", staged: false })
        const unicode = gitChangeDomId("local-file", { path: "資料/é.ts", staged: true })
        expect(slash).toBe("local-file-c-a_2fb.ts")
        expect(underscoreHex).toBe("local-file-c-a_5f2fb.ts")
        expect(slash).not.toBe(underscoreHex)
        expect(unicode).toBe("local-file-s-_e8_b3_87_e6_96_99_2f_c3_a9.ts")
        expect(gitChangeDomId("local-file", { path: "a/b.ts", staged: true }))
            .toBe("local-file-s-a_2fb.ts")
    })
})

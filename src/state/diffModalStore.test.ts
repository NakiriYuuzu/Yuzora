import { beforeEach, describe, expect, it } from "vitest"

import type { CommitFileChange } from "@/lib/types"
import {
    commitLikeFrom,
    useDiffModalStore,
    type WorktreeDiffFile
} from "./diffModalStore"

const wtFile = (path: string, staged = false): WorktreeDiffFile => ({
    path,
    status: "M",
    staged
})

const cf = (path: string, over: Partial<CommitFileChange> = {}): CommitFileChange => ({
    status: "M",
    path,
    oldPath: null,
    additions: 1,
    deletions: 0,
    binary: false,
    ...over
})

describe("diffModalStore", () => {
    beforeEach(() => {
        // Reset to a known closed state (mode default unified).
        useDiffModalStore.setState({
            open: false,
            source: null,
            activeIndex: 0,
            mode: "unified"
        })
    })

    it("openWorktree opens with the file list and selects activePath", () => {
        useDiffModalStore
            .getState()
            .openWorktree([wtFile("a.ts"), wtFile("b.ts"), wtFile("c.ts")], "b.ts")
        const s = useDiffModalStore.getState()
        expect(s.open).toBe(true)
        expect(s.source?.type).toBe("worktree")
        expect(s.activeIndex).toBe(1)
    })

    it("openWorktree defaults to index 0 when activePath is absent or unknown", () => {
        useDiffModalStore.getState().openWorktree([wtFile("a.ts")], "missing.ts")
        expect(useDiffModalStore.getState().activeIndex).toBe(0)
        useDiffModalStore.getState().openWorktree([wtFile("a.ts")])
        expect(useDiffModalStore.getState().activeIndex).toBe(0)
    })

    it("openWorktree pins the exact side for an MM file via { path, staged } (F2)", () => {
        // A partially-staged file has two rows with the same path. A string active
        // matches the first (staged) row; an object active pins the requested
        // side so the sidebar CHANGED row lands on the unstaged one.
        const files = [wtFile("mm.ts", true), wtFile("mm.ts", false)]
        // string → first matching row (staged, index 0).
        useDiffModalStore.getState().openWorktree(files, "mm.ts")
        expect(useDiffModalStore.getState().activeIndex).toBe(0)
        // object pinning the unstaged side → index 1.
        useDiffModalStore.getState().openWorktree(files, { path: "mm.ts", staged: false })
        expect(useDiffModalStore.getState().activeIndex).toBe(1)
        // object pinning the staged side → index 0.
        useDiffModalStore.getState().openWorktree(files, { path: "mm.ts", staged: true })
        expect(useDiffModalStore.getState().activeIndex).toBe(0)
    })

    it("openCommit opens with the commit source and given activeIndex", () => {
        useDiffModalStore.getState().openCommit(
            {
                hash: "h".repeat(40),
                shortHash: "hhhhhhh",
                subject: "subject",
                parents: ["p".repeat(40)],
                files: [cf("a.ts"), cf("b.ts")]
            },
            1
        )
        const s = useDiffModalStore.getState()
        expect(s.open).toBe(true)
        expect(s.source).toMatchObject({ type: "commit", shortHash: "hhhhhhh", subject: "subject" })
        expect(s.activeIndex).toBe(1)
    })

    it("openCommit defaults activeIndex to 0", () => {
        useDiffModalStore.getState().openCommit({
            hash: "h".repeat(40),
            shortHash: "hhhhhhh",
            subject: "s",
            parents: [],
            files: [cf("a.ts")]
        })
        expect(useDiffModalStore.getState().activeIndex).toBe(0)
    })

    it("setActive updates the active index", () => {
        useDiffModalStore.getState().openWorktree([wtFile("a.ts"), wtFile("b.ts")])
        useDiffModalStore.getState().setActive(1)
        expect(useDiffModalStore.getState().activeIndex).toBe(1)
    })

    it("setMode changes the mode", () => {
        useDiffModalStore.getState().setMode("split")
        expect(useDiffModalStore.getState().mode).toBe("split")
    })

    it("close clears source but keeps the mode preference sticky", () => {
        useDiffModalStore.getState().setMode("split")
        useDiffModalStore.getState().openWorktree([wtFile("a.ts")])
        useDiffModalStore.getState().close()
        const s = useDiffModalStore.getState()
        expect(s.open).toBe(false)
        expect(s.source).toBeNull()
        expect(s.activeIndex).toBe(0)
        // mode is NOT reset on close.
        expect(s.mode).toBe("split")
    })

    it("commitLikeFrom pairs commit identity with detail.files", () => {
        const like = commitLikeFrom(
            { hash: "h".repeat(40), shortHash: "hhhhhhh", subject: "s", parents: ["p"] },
            { files: [cf("a.ts")] }
        )
        expect(like).toEqual({
            hash: "h".repeat(40),
            shortHash: "hhhhhhh",
            subject: "s",
            parents: ["p"],
            files: [cf("a.ts")]
        })
    })
})

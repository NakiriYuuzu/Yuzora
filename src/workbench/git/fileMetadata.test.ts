import { expect, it } from "vitest"
import type { GitStatus } from "@/lib/types"
import { worktreeFileMetadata } from "./fileRows"

const snapshot = (): GitStatus => ({
    branch: "main", headOid: "a", detached: false, upstream: null, ahead: 0, behind: 0,
    staged: [{ path: "both.ts", status: "M", origPath: null }],
    unstaged: [{ path: "both.ts", status: "D", origPath: null }, { path: "working.ts", status: "M", origPath: null }],
    untracked: ["new.ts"], conflicted: [], inProgress: null
})

it("reuses a snapshot index, preserves staged priority, and handles null", () => {
    const status = snapshot()
    const index = worktreeFileMetadata(status)
    expect(worktreeFileMetadata(status)).toBe(index)
    expect(index.get("both.ts")).toMatchObject({ staged: true, status: "M" })
    expect(index.get("working.ts")).toMatchObject({ staged: false, status: "M" })
    expect(index.get("new.ts")).toMatchObject({ staged: false, status: "?" })
    expect(worktreeFileMetadata(null).size).toBe(0)
})

it("indexes a replacement snapshot without leaking old status metadata", () => {
    const previous = snapshot()
    const next = { ...previous, staged: [] }
    expect(worktreeFileMetadata(next)).not.toBe(worktreeFileMetadata(previous))
    expect(worktreeFileMetadata(next).get("both.ts")).toMatchObject({ staged: false, status: "D" })
    expect(worktreeFileMetadata(previous).get("both.ts")).toMatchObject({ staged: true, status: "M" })
})

import { create } from "zustand"

import type { CommitDetail, CommitFileChange, GradedText } from "@/lib/types"

export type DiffMode = "unified" | "split"

// A worktree file row. Mirrors the GitStatus buckets flattened by the Local
// changes tab: `staged` records which side git diffs against (staged↔working).
export interface WorktreeDiffFile {
    path: string
    status: string
    staged: boolean
}

// Modal sources. Worktree carries a flat file list; commit carries the
// hash/parents needed to load each file's old/new sides plus the file changes.
// Text carries preloaded blobs, so the modal can render non-git diffs directly.
export type DiffModalSource =
    | { type: "worktree"; files: WorktreeDiffFile[] }
    | {
          type: "commit"
          hash: string
          shortHash: string
          subject: string
          parents: string[]
          files: CommitFileChange[]
      }
    | { type: "text"; title: string; original: GradedText; modified: GradedText }

// The commit detail shape openCommit accepts — a subset of CommitDetail plus the
// identity fields the header/loading need (from the selected LogCommit).
export interface CommitLike {
    hash: string
    shortHash: string
    subject: string
    parents: string[]
    files: CommitFileChange[]
}

interface DiffModalState {
    open: boolean
    source: DiffModalSource | null
    activeIndex: number
    mode: DiffMode
    // active locates the row to open on. A string matches by path only (legacy
    // semantics: first row with that path — staged side wins when a path has both
    // an MM staged and unstaged row). An object pins the exact side via `staged`,
    // which the sidebar CHANGED/STAGED rows need so a partially-staged (MM) file
    // opens on the clicked side rather than always the staged one.
    openWorktree: (
        files: WorktreeDiffFile[],
        active?: { path: string; staged: boolean } | string
    ) => void
    openCommit: (commit: CommitLike, activeIndex?: number) => void
    openText: (title: string, original: GradedText, modified: GradedText) => void
    setActive: (index: number) => void
    setMode: (mode: DiffMode) => void
    close: () => void
}

// mode is NOT reset on close — the unified/split preference sticks across opens
// (matches the Local changes tab keeping its own toggle). close() clears source
// so the modal has nothing stale to render while animating out.
export const useDiffModalStore = create<DiffModalState>((set) => ({
    open: false,
    source: null,
    activeIndex: 0,
    mode: "unified",
    openWorktree: (files, active) => {
        const idx =
            typeof active === "string"
                ? files.findIndex((f) => f.path === active)
                : active
                  ? files.findIndex((f) => f.path === active.path && f.staged === active.staged)
                  : -1
        set({
            open: true,
            source: { type: "worktree", files },
            activeIndex: idx >= 0 ? idx : 0
        })
    },
    openCommit: (commit, activeIndex = 0) =>
        set({
            open: true,
            source: {
                type: "commit",
                hash: commit.hash,
                shortHash: commit.shortHash,
                subject: commit.subject,
                parents: commit.parents,
                files: commit.files
            },
            activeIndex
        }),
    openText: (title, original, modified) =>
        set({
            open: true,
            source: { type: "text", title, original, modified },
            activeIndex: 0
        }),
    setActive: (index) => set({ activeIndex: index }),
    setMode: (mode) => set({ mode }),
    close: () => set({ open: false, source: null, activeIndex: 0 })
}))

// Build a CommitLike from the log store's selected commit + detail. Kept here so
// the three call sites (GitPanel wiring) share one mapping. detail.files is the
// authoritative changed-file list; the identity fields come from the commit.
export function commitLikeFrom(
    commit: { hash: string; shortHash: string; subject: string; parents: string[] },
    detail: Pick<CommitDetail, "files">
): CommitLike {
    return {
        hash: commit.hash,
        shortHash: commit.shortHash,
        subject: commit.subject,
        parents: commit.parents,
        files: detail.files
    }
}

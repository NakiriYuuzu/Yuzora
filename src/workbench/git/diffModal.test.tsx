import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"
import { act, cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react"

import type { CommitFileChange, DiffContent, FileAtRevResult } from "@/lib/types"

// ipc mocks. gitDiffContent backs worktree files; gitFileAtRev backs commit
// files (spied for old/new load-parameter assertions).
const gitDiffContent = vi.fn<(path: string, staged: boolean) => Promise<DiffContent>>(
    async () => ({
        original: { kind: "full", content: "old\n" },
        modified: { kind: "full", content: "new\n" }
    })
)
const gitFileAtRev = vi.fn<(rev: string, path: string) => Promise<FileAtRevResult>>(
    async () => ({ kind: "full", content: "content\n" })
)

vi.mock("@/lib/ipc", () => ({
    gitDiffContent: (p: string, s: boolean) => gitDiffContent(p, s),
    gitFileAtRev: (r: string, p: string) => gitFileAtRev(r, p)
}))

const { DiffModal } = await import("./DiffModal")
const { useDiffModalStore } = await import("@/state/diffModalStore")

const cf = (path: string, over: Partial<CommitFileChange> = {}): CommitFileChange => ({
    status: "M",
    path,
    oldPath: null,
    additions: 1,
    deletions: 0,
    binary: false,
    ...over
})

const full = (content: string) => ({ kind: "full" as const, content })

function reset() {
    useDiffModalStore.setState({ open: false, source: null, activeIndex: 0, mode: "unified" })
}

beforeEach(() => {
    reset()
    gitDiffContent.mockClear()
    gitFileAtRev.mockClear()
    gitDiffContent.mockResolvedValue({
        original: { kind: "full", content: "old\n" },
        modified: { kind: "full", content: "new\n" }
    })
    gitFileAtRev.mockResolvedValue({ kind: "full", content: "content\n" })
})
afterEach(() => cleanup())

describe("DiffModal — closed", () => {
    it("renders nothing when closed", () => {
        render(<DiffModal />)
        expect(screen.queryByText(/Diff ·/)).toBeNull()
    })
})

describe("DiffModal — text source", () => {
    it("renders the provided blobs in split mode without git IPC", async () => {
        const { container } = render(<DiffModal />)

        act(() => {
            useDiffModalStore.getState().setMode("split")
            useDiffModalStore.getState().openText("src/a.ts", full("old\n"), full("new\n"))
        })

        expect(screen.getAllByText("Diff · src/a.ts").length).toBeGreaterThan(0)
        expect(screen.getByRole("button", { name: /a\.ts/ })).toBeInTheDocument()
        await waitFor(() => expect(container.querySelectorAll(".cm-editor").length).toBe(2))
        expect(gitDiffContent).not.toHaveBeenCalled()
        expect(gitFileAtRev).not.toHaveBeenCalled()
    })
})

describe("DiffModal — worktree source", () => {
    it("renders the Working tree header, file-count sub, and file list", async () => {
        render(<DiffModal />)
        act(() => {
            useDiffModalStore.getState().openWorktree(
                [
                    { path: "src/a.ts", status: "M", staged: false },
                    { path: "b.ts", status: "A", staged: true }
                ],
                "src/a.ts"
            )
        })
        // "Diff · Working tree" renders in both the visible header and the
        // sr-only dialog title.
        expect(screen.getAllByText("Diff · Working tree").length).toBeGreaterThan(0)
        // 2 changed files → the sub label appears (header + list header).
        expect(screen.getAllByText("2 changed files").length).toBeGreaterThan(0)
        expect(screen.getByText("a.ts")).toBeInTheDocument()
        expect(screen.getByText("b.ts")).toBeInTheDocument()
        // Active file (src/a.ts) loads its worktree diff (staged=false).
        await waitFor(() => expect(gitDiffContent).toHaveBeenCalledWith("src/a.ts", false))
    })

    it("clicking a file row loads that file's diff (staged side respected)", async () => {
        render(<DiffModal />)
        act(() => {
            useDiffModalStore.getState().openWorktree(
                [
                    { path: "a.ts", status: "M", staged: false },
                    { path: "b.ts", status: "M", staged: true }
                ],
                "a.ts"
            )
        })
        await waitFor(() => expect(gitDiffContent).toHaveBeenCalledWith("a.ts", false))
        fireEvent.click(screen.getByRole("button", { name: /b\.ts/ }))
        await waitFor(() => expect(gitDiffContent).toHaveBeenCalledWith("b.ts", true))
    })

    it("MM file's two rows each load their own side without cache cross-pollution (F2)", async () => {
        // A partially-staged file appears twice with the same path — staged and
        // unstaged. The per-open cache keys by side (s:/c:), so clicking the
        // second row must NOT serve the first row's cached (wrong-side) diff.
        // Give each side distinct content so a mix-up would surface.
        gitDiffContent.mockImplementation(async (_p: string, staged: boolean) => ({
            original: { kind: "full", content: staged ? "STAGED old\n" : "UNSTAGED old\n" },
            modified: { kind: "full", content: staged ? "STAGED new\n" : "UNSTAGED new\n" }
        }))
        render(<DiffModal />)
        act(() => {
            useDiffModalStore.getState().openWorktree([
                { path: "mm.ts", status: "M", staged: true },
                { path: "mm.ts", status: "M", staged: false }
            ])
        })
        // Row 0 (staged) is active by default → loads the staged side.
        await waitFor(() => expect(gitDiffContent).toHaveBeenCalledWith("mm.ts", true))
        gitDiffContent.mockClear()
        // Select row 1 (unstaged) → must load the unstaged side (cache miss on the
        // c:mm.ts key, not a hit on the already-loaded s:mm.ts key).
        act(() => useDiffModalStore.getState().setActive(1))
        await waitFor(() => expect(gitDiffContent).toHaveBeenCalledWith("mm.ts", false))
        // Selecting the staged row again serves its own cached side — no reload.
        gitDiffContent.mockClear()
        act(() => useDiffModalStore.getState().setActive(0))
        await Promise.resolve()
        expect(gitDiffContent).not.toHaveBeenCalled()
    })
})

describe("DiffModal — commit source", () => {
    const openCommit = (files: CommitFileChange[], parents: string[], activeIndex = 0) =>
        act(() => {
            useDiffModalStore.getState().openCommit(
                {
                    hash: "hash".padEnd(40, "0"),
                    shortHash: "hash000",
                    subject: "fix: the thing",
                    parents,
                    files
                },
                activeIndex
            )
        })

    it("renders the shortHash title and subject sub", () => {
        render(<DiffModal />)
        openCommit([cf("a.ts")], ["parent".padEnd(40, "0")])
        // "Diff · hash000" renders in the visible header and the sr-only title.
        expect(screen.getAllByText("Diff · hash000").length).toBeGreaterThan(0)
        // subject shows in both header sub and file-list header.
        expect(screen.getAllByText("fix: the thing").length).toBeGreaterThan(0)
    })

    it("loads old from first parent and new from the commit", async () => {
        render(<DiffModal />)
        const parent = "parent".padEnd(40, "0")
        const hash = "hash".padEnd(40, "0")
        openCommit([cf("a.ts")], [parent])
        await waitFor(() => {
            expect(gitFileAtRev).toHaveBeenCalledWith(parent, "a.ts")
            expect(gitFileAtRev).toHaveBeenCalledWith(hash, "a.ts")
        })
    })

    it("resolves the old side against oldPath for a rename", async () => {
        render(<DiffModal />)
        const parent = "parent".padEnd(40, "0")
        const hash = "hash".padEnd(40, "0")
        openCommit([cf("new.ts", { status: "R", oldPath: "old.ts" })], [parent])
        await waitFor(() => {
            // old side uses oldPath, new side uses the current path.
            expect(gitFileAtRev).toHaveBeenCalledWith(parent, "old.ts")
            expect(gitFileAtRev).toHaveBeenCalledWith(hash, "new.ts")
        })
    })

    it("root commit (no parent) loads only the new side", async () => {
        render(<DiffModal />)
        const hash = "hash".padEnd(40, "0")
        openCommit([cf("a.ts", { status: "A" })], [])
        await waitFor(() => expect(gitFileAtRev).toHaveBeenCalledWith(hash, "a.ts"))
        // Only one call — no parent to load an old side from.
        expect(gitFileAtRev).toHaveBeenCalledTimes(1)
    })

    it("missing (A file old side) is treated as empty text and still renders a diff", async () => {
        gitFileAtRev.mockImplementation(async (rev: string) =>
            rev.startsWith("parent")
                ? { kind: "missing" }
                : { kind: "full", content: "added\n" }
        )
        const { container } = render(<DiffModal />)
        openCommit([cf("a.ts", { status: "A" })], ["parent".padEnd(40, "0")])
        // missing→"" old side; new side has content → CodeMirror mounts (not the
        // undisplayable EmptyState).
        await waitFor(() => expect(container.querySelector(".cm-editor")).not.toBeNull())
        expect(screen.queryByText("無法顯示 diff")).toBeNull()
    })

    it("binary side shows the undisplayable EmptyState", async () => {
        gitFileAtRev.mockResolvedValue({ kind: "binary" })
        render(<DiffModal />)
        openCommit([cf("img.png", { binary: true })], ["parent".padEnd(40, "0")])
        await waitFor(() => expect(screen.getByText("無法顯示 diff")).toBeInTheDocument())
    })

    it("switching active file via the store loads the newly-active file", async () => {
        render(<DiffModal />)
        const parent = "parent".padEnd(40, "0")
        openCommit([cf("a.ts"), cf("b.ts")], [parent], 0)
        await waitFor(() => expect(gitFileAtRev).toHaveBeenCalledWith(parent, "a.ts"))
        act(() => useDiffModalStore.getState().setActive(1))
        await waitFor(() => expect(gitFileAtRev).toHaveBeenCalledWith(parent, "b.ts"))
    })
})

describe("DiffModal — mode toggle + close", () => {
    it("Unified/Split toggle drives store mode and swaps the editor layout", async () => {
        const { container } = render(<DiffModal />)
        act(() => {
            useDiffModalStore.getState().openWorktree([{ path: "a.ts", status: "M", staged: false }], "a.ts")
        })
        await waitFor(() => expect(container.querySelector(".cm-editor")).not.toBeNull())
        // unified → one editor.
        expect(container.querySelectorAll(".cm-editor").length).toBe(1)
        fireEvent.click(screen.getByRole("button", { name: "Split" }))
        expect(useDiffModalStore.getState().mode).toBe("split")
        await waitFor(() => expect(container.querySelectorAll(".cm-editor").length).toBe(2))
    })

    it("close button closes the modal", () => {
        render(<DiffModal />)
        act(() => {
            useDiffModalStore.getState().openWorktree([{ path: "a.ts", status: "M", staged: false }], "a.ts")
        })
        fireEvent.click(screen.getByRole("button", { name: "Close" }))
        expect(useDiffModalStore.getState().open).toBe(false)
    })

    it("Escape closes the modal", () => {
        render(<DiffModal />)
        act(() => {
            useDiffModalStore.getState().openWorktree([{ path: "a.ts", status: "M", staged: false }], "a.ts")
        })
        fireEvent.keyDown(document.body, { key: "Escape" })
        expect(useDiffModalStore.getState().open).toBe(false)
    })

    it("clicking the overlay closes the modal", () => {
        const { container } = render(<DiffModal />)
        act(() => {
            useDiffModalStore.getState().openWorktree([{ path: "a.ts", status: "M", staged: false }], "a.ts")
        })
        // The overlay carries the design's translucent-ink background class.
        const overlay = container.ownerDocument.body.querySelector(
            ".bg-\\[rgba\\(27\\,26\\,23\\,0\\.34\\)\\]"
        ) as HTMLElement
        expect(overlay).toBeTruthy()
        fireEvent.click(overlay)
        expect(useDiffModalStore.getState().open).toBe(false)
    })

    it("selected file row gets the active styling", async () => {
        render(<DiffModal />)
        act(() => {
            useDiffModalStore.getState().openWorktree(
                [
                    { path: "a.ts", status: "M", staged: false },
                    { path: "b.ts", status: "M", staged: false }
                ],
                "b.ts"
            )
        })
        const row = screen.getByRole("button", { name: /b\.ts/ })
        expect(row.className).toContain("bg-(--yz-active)")
        const other = screen.getByRole("button", { name: /a\.ts/ })
        expect(other.className).not.toContain("bg-(--yz-active)")
        await waitFor(() => expect(within(row).getByText("b.ts")).toBeInTheDocument())
    })
})

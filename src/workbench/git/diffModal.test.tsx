import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"
import { act, cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react"

import type { CommitFileChange, DiffContent, FileAtRevResult } from "@/lib/types"

// ipc mocks. gitDiffContent backs worktree files; gitFileAtRev backs commit
// files (spied for old/new load-parameter assertions).
const gitDiffContent = vi.fn<(root: string, path: string, staged: boolean, origPath?: string | null) => Promise<DiffContent>>(
    async () => ({
        original: { kind: "full", content: "old\n" },
        modified: { kind: "full", content: "new\n" }
    })
)
const gitFileAtRev = vi.fn<(root: string, rev: string, path: string) => Promise<FileAtRevResult>>(
    async () => ({ kind: "full", content: "content\n" })
)

vi.mock("@/lib/ipc", () => ({
    gitDiffContent: (root: string, p: string, s: boolean, orig?: string | null) => gitDiffContent(root, p, s, orig),
    gitFileAtRev: (root: string, r: string, p: string) => gitFileAtRev(root, r, p)
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
        expect(screen.getByRole("option", { name: /a\.ts/ })).toBeInTheDocument()
        await waitFor(() => expect(container.querySelectorAll(".cm-editor").length).toBe(2))
        expect(gitDiffContent).not.toHaveBeenCalled()
        expect(gitFileAtRev).not.toHaveBeenCalled()
    })
})

describe("DiffModal — worktree source", () => {
    it("renders the Working tree header, file-count sub, and file list", async () => {
        render(<DiffModal />)
        act(() => {
            useDiffModalStore.getState().openWorktree("/w",
                [
                    { path: "src/a.ts", origPath: null, status: "M", staged: false },
                    { path: "b.ts", origPath: null, status: "A", staged: true }
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
        await waitFor(() => expect(gitDiffContent).toHaveBeenCalledWith("/w", "src/a.ts", false, null))
    })

    it("clicking a file row loads that file's diff (staged side respected)", async () => {
        render(<DiffModal />)
        act(() => {
            useDiffModalStore.getState().openWorktree("/w",
                [
                    { path: "a.ts", origPath: null, status: "M", staged: false },
                    { path: "b.ts", origPath: null, status: "M", staged: true }
                ],
                "a.ts"
            )
        })
        await waitFor(() => expect(gitDiffContent).toHaveBeenCalledWith("/w", "a.ts", false, null))
        fireEvent.click(screen.getByRole("option", { name: /b\.ts/ }))
        await waitFor(() => expect(gitDiffContent).toHaveBeenCalledWith("/w", "b.ts", true, null))
    })

    it("MM file's two rows each load their own side without cache cross-pollution (F2)", async () => {
        // A partially-staged file appears twice with the same path — staged and
        // unstaged. The per-open cache keys by side (s:/c:), so clicking the
        // second row must NOT serve the first row's cached (wrong-side) diff.
        // Give each side distinct content so a mix-up would surface.
        gitDiffContent.mockImplementation(async (_root: string, _p: string, staged: boolean) => ({
            original: { kind: "full", content: staged ? "STAGED old\n" : "UNSTAGED old\n" },
            modified: { kind: "full", content: staged ? "STAGED new\n" : "UNSTAGED new\n" }
        }))
        render(<DiffModal />)
        act(() => {
            useDiffModalStore.getState().openWorktree("/w", [
                { path: "mm.ts", origPath: null, status: "M", staged: true },
                { path: "mm.ts", origPath: null, status: "M", staged: false }
            ])
        })
        // Row 0 (staged) is active by default → loads the staged side.
        await waitFor(() => expect(gitDiffContent).toHaveBeenCalledWith("/w", "mm.ts", true, null))
        gitDiffContent.mockClear()
        // Select row 1 (unstaged) → must load the unstaged side (cache miss on the
        // c:mm.ts key, not a hit on the already-loaded s:mm.ts key).
        act(() => useDiffModalStore.getState().setActive(1))
        await waitFor(() => expect(gitDiffContent).toHaveBeenCalledWith("/w", "mm.ts", false, null))
        // Selecting the staged row again serves its own cached side — no reload.
        gitDiffContent.mockClear()
        act(() => useDiffModalStore.getState().setActive(0))
        await Promise.resolve()
        expect(gitDiffContent).not.toHaveBeenCalled()
    })

    it("keeps the newest A request when switching A → B → A with the same cache key", async () => {
        let resolveOldA: ((value: DiffContent) => void) | null = null
        let resolveB: ((value: DiffContent) => void) | null = null
        let resolveNewA: ((value: DiffContent) => void) | null = null
        gitDiffContent
            .mockImplementationOnce(() => new Promise((resolve) => { resolveOldA = resolve }))
            .mockImplementationOnce(() => new Promise((resolve) => { resolveB = resolve }))
            .mockImplementationOnce(() => new Promise((resolve) => { resolveNewA = resolve }))
        const { container } = render(<DiffModal />)
        act(() => useDiffModalStore.getState().openWorktree("/w", [
            { path: "a.ts", origPath: null, status: "M", staged: false },
            { path: "b.ts", origPath: null, status: "M", staged: false }
        ]))
        await waitFor(() => expect(gitDiffContent).toHaveBeenNthCalledWith(1, "/w", "a.ts", false, null))
        act(() => useDiffModalStore.getState().setActive(1))
        await waitFor(() => expect(gitDiffContent).toHaveBeenNthCalledWith(2, "/w", "b.ts", false, null))
        act(() => useDiffModalStore.getState().setActive(0))
        await waitFor(() => expect(gitDiffContent).toHaveBeenNthCalledWith(3, "/w", "a.ts", false, null))

        await act(async () => {
            resolveNewA?.({ original: full("newest old\n"), modified: full("newest A\n") })
            await Promise.resolve()
        })
        const editorText = () => Array.from(container.querySelectorAll(".cm-content"), (node) => node.textContent).join("\n")
        await waitFor(() => expect(editorText()).toContain("newest A"))
        await act(async () => {
            resolveOldA?.({ original: full("stale old\n"), modified: full("stale A\n") })
            await Promise.resolve()
        })
        expect(editorText()).not.toContain("stale A")

        act(() => useDiffModalStore.getState().setActive(1))
        await act(async () => {
            resolveB?.({ original: full("b old\n"), modified: full("B\n") })
            await Promise.resolve()
        })
        await waitFor(() => expect(gitDiffContent).toHaveBeenCalledTimes(4))
        gitDiffContent.mockClear()
        act(() => useDiffModalStore.getState().setActive(0))
        await waitFor(() => expect(editorText()).toContain("newest A"))
        expect(gitDiffContent).not.toHaveBeenCalled()
    })

    it("hides previous content synchronously when the same path/side source is replaced", async () => {
        gitDiffContent.mockResolvedValueOnce({
            original: full("old A\n"),
            modified: full("content A\n")
        })
        const { container } = render(<DiffModal />)
        act(() => {
            useDiffModalStore.getState().openWorktree("/w", [
                { path: "a.ts", origPath: null, status: "M", staged: false }
            ])
        })
        const editorText = () =>
            Array.from(container.querySelectorAll(".cm-content"), (node) => node.textContent).join("\n")
        await waitFor(() => expect(editorText()).toContain("content A"))
        const generation = useDiffModalStore.getState().sourceGeneration

        // Next open hangs so any residual content would be from the previous source.
        gitDiffContent.mockImplementation(() => new Promise(() => {}))
        act(() => {
            useDiffModalStore.getState().openWorktree("/w", [
                { path: "a.ts", origPath: null, status: "M", staged: false }
            ])
        })
        expect(useDiffModalStore.getState().sourceGeneration).toBe(generation + 1)
        expect(editorText()).not.toContain("content A")
        expect(screen.getByText(/Loading diff/i)).toBeInTheDocument()
    })
})

describe("DiffModal — commit source", () => {
    const openCommit = (files: CommitFileChange[], parents: string[], activeIndex = 0) =>
        act(() => {
            useDiffModalStore.getState().openCommit(
                "/w",
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
            expect(gitFileAtRev).toHaveBeenCalledWith("/w", parent, "a.ts")
            expect(gitFileAtRev).toHaveBeenCalledWith("/w", hash, "a.ts")
        })
    })

    it("resolves the old side against oldPath for a rename", async () => {
        render(<DiffModal />)
        const parent = "parent".padEnd(40, "0")
        const hash = "hash".padEnd(40, "0")
        openCommit([cf("new.ts", { status: "R", oldPath: "old.ts" })], [parent])
        await waitFor(() => {
            // old side uses oldPath, new side uses the current path.
            expect(gitFileAtRev).toHaveBeenCalledWith("/w", parent, "old.ts")
            expect(gitFileAtRev).toHaveBeenCalledWith("/w", hash, "new.ts")
        })
    })

    it("root commit (no parent) loads only the new side", async () => {
        render(<DiffModal />)
        const hash = "hash".padEnd(40, "0")
        openCommit([cf("a.ts", { status: "A" })], [])
        await waitFor(() => expect(gitFileAtRev).toHaveBeenCalledWith("/w", hash, "a.ts"))
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
        expect(screen.queryByText("Diff unavailable")).toBeNull()
    })

    it("binary side shows the undisplayable EmptyState", async () => {
        gitFileAtRev.mockResolvedValue({ kind: "binary" })
        render(<DiffModal />)
        openCommit([cf("img.png", { binary: true })], ["parent".padEnd(40, "0")])
        await waitFor(() => expect(screen.getByText("Diff unavailable")).toBeInTheDocument())
    })

    it("switching active file via the store loads the newly-active file", async () => {
        render(<DiffModal />)
        const parent = "parent".padEnd(40, "0")
        openCommit([cf("a.ts"), cf("b.ts")], [parent], 0)
        await waitFor(() => expect(gitFileAtRev).toHaveBeenCalledWith("/w", parent, "a.ts"))
        act(() => useDiffModalStore.getState().setActive(1))
        await waitFor(() => expect(gitFileAtRev).toHaveBeenCalledWith("/w", parent, "b.ts"))
    })
})

describe("DiffModal — mode toggle + close", () => {
    it("Unified/Split toggle drives store mode and swaps the editor layout", async () => {
        const { container } = render(<DiffModal />)
        act(() => {
            useDiffModalStore.getState().openWorktree("/w", [{ path: "a.ts", origPath: null, status: "M", staged: false }], "a.ts")
        })
        await waitFor(() => expect(container.querySelector(".cm-editor")).not.toBeNull())
        // unified → one editor.
        expect(container.querySelectorAll(".cm-editor").length).toBe(1)
        fireEvent.click(screen.getByRole("radio", { name: "Split" }))
        expect(useDiffModalStore.getState().mode).toBe("split")
        await waitFor(() => expect(container.querySelectorAll(".cm-editor").length).toBe(2))
    })

    it("close button closes the modal", () => {
        render(<DiffModal />)
        act(() => {
            useDiffModalStore.getState().openWorktree("/w", [{ path: "a.ts", origPath: null, status: "M", staged: false }], "a.ts")
        })
        fireEvent.click(screen.getByRole("button", { name: "Close" }))
        expect(useDiffModalStore.getState().open).toBe(false)
    })

    it("Escape closes the modal", () => {
        render(<DiffModal />)
        act(() => {
            useDiffModalStore.getState().openWorktree("/w", [{ path: "a.ts", origPath: null, status: "M", staged: false }], "a.ts")
        })
        fireEvent.keyDown(document.body, { key: "Escape" })
        expect(useDiffModalStore.getState().open).toBe(false)
    })

    it("clicking the overlay closes the modal", () => {
        const { container } = render(<DiffModal />)
        act(() => {
            useDiffModalStore.getState().openWorktree("/w", [{ path: "a.ts", origPath: null, status: "M", staged: false }], "a.ts")
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
            useDiffModalStore.getState().openWorktree("/w",
                [
                    { path: "a.ts", origPath: null, status: "M", staged: false },
                    { path: "b.ts", origPath: null, status: "M", staged: false }
                ],
                "b.ts"
            )
        })
        const row = screen.getByRole("option", { name: /b\.ts/ })
        expect(row.className).toContain("bg-(--yz-active)")
        const other = screen.getByRole("option", { name: /a\.ts/ })
        expect(other.className).not.toContain("bg-(--yz-active)")
        await waitFor(() => expect(within(row).getByText("b.ts")).toBeInTheDocument())
    })

    it("groups worktree MM sides and supports listbox navigation", async () => {
        render(<DiffModal />)
        act(() => {
            useDiffModalStore.getState().openWorktree("/w", [
                { path: "mm.ts", origPath: null, status: "M", staged: true },
                { path: "mm.ts", origPath: null, status: "M", staged: false },
                { path: "only.ts", origPath: null, status: "M", staged: false }
            ])
        })
        expect(screen.getAllByText("Staged").length).toBeGreaterThan(0)
        expect(screen.getAllByText("Unstaged").length).toBeGreaterThan(0)
        expect(screen.getByRole("option", { name: /mm\.ts \(Staged\)/ })).toBeInTheDocument()
        const staged = screen.getByRole("option", { name: /mm\.ts \(Staged\)/ })
        fireEvent.keyDown(staged, { key: "ArrowDown" })
        await waitFor(() => expect(useDiffModalStore.getState().activeIndex).toBe(1))
    })

    it("filters the file list without changing cache identity", async () => {
        render(<DiffModal />)
        act(() => {
            useDiffModalStore.getState().openWorktree(
                "/w",
                Array.from({ length: 16 }, (_, i) => ({
                    path: `file${i}.ts`,
                    origPath: null,
                    status: "M",
                    staged: false
                }))
            )
        })
        const filter = screen.getByLabelText("Filter files")
        fireEvent.change(filter, { target: { value: "file15" } })
        expect(screen.getByRole("option", { name: /file15\.ts/ })).toBeInTheDocument()
        expect(screen.queryByRole("option", { name: /file0\.ts/ })).toBeNull()
        expect(useDiffModalStore.getState().activeIndex).toBe(0)
        await waitFor(() => expect(gitDiffContent).toHaveBeenCalledWith("/w", "file0.ts", false, null))
    })

    it("keeps a mounted roving tabindex when the active file is filtered out", () => {
        render(<DiffModal />)
        act(() => {
            useDiffModalStore.getState().openWorktree(
                "/w",
                Array.from({ length: 16 }, (_, i) => ({
                    path: `file${i}.ts`,
                    origPath: null,
                    status: "M",
                    staged: false
                })),
                "file1.ts"
            )
        })
        expect(useDiffModalStore.getState().activeIndex).toBe(1)
        fireEvent.change(screen.getByLabelText("Filter files"), { target: { value: "file0" } })
        const visible = screen.getByRole("option", { name: /file0\.ts/ })
        expect(visible).toHaveAttribute("tabindex", "0")
        expect(screen.getByRole("listbox")).toHaveAttribute("aria-activedescendant", "diff-file-0")
        expect(useDiffModalStore.getState().activeIndex).toBe(1)
        expect(screen.queryByRole("option", { name: /file1\.ts/ })).toBeNull()
    })

    it("syncs collapsed chrome from the panel API, not only the toggle button", async () => {
        render(<DiffModal />)
        act(() => {
            useDiffModalStore.getState().openWorktree("/w", [
                { path: "a.ts", origPath: null, status: "M", staged: false }
            ])
        })
        const panel = screen.getByTestId("diff-files") as HTMLElement & {
            __yzPanel?: { collapse: () => void; expand: () => void }
        }
        expect(panel.__yzPanel).toBeTruthy()
        expect(screen.getByTestId("diff-file-panel-content").contains(screen.getByRole("button", { name: "Collapse file list" }))).toBe(false)
        act(() => panel.__yzPanel?.collapse())
        await waitFor(() => expect(screen.getByRole("button", { name: "Expand file list" })).toBeInTheDocument())
        expect(screen.getByTestId("diff-file-panel-content").contains(screen.getByRole("button", { name: "Expand file list" }))).toBe(false)
        expect(panel).toHaveAttribute("data-files-collapsed", "true")
        act(() => panel.__yzPanel?.expand())
        await waitFor(() => expect(screen.getByRole("button", { name: "Collapse file list" })).toHaveAttribute("aria-expanded", "true"))
        expect(screen.getByTestId("diff-files-handle")).toBeInTheDocument()
        expect(screen.getByTestId("diff-files-handle")).not.toHaveAttribute("disabled")
        expect(screen.getByTestId("diff-modal-title").className).toMatch(/min-w-0/)
        expect(screen.getByTestId("diff-modal-title").className).toMatch(/overflow-hidden/)
        expect(screen.getByRole("button", { name: "Collapse file list" })).toHaveAttribute("aria-controls", "diff-file-panel-content")
    })

    it("hides a collapsed large file list from accessibility and moves focus to expand", async () => {
        render(<DiffModal />)
        act(() => {
            useDiffModalStore.getState().openWorktree(
                "/w",
                Array.from({ length: 16 }, (_, i) => ({
                    path: `file${i}.ts`,
                    origPath: null,
                    status: "M",
                    staged: false
                })),
                "file3.ts"
            )
        })
        expect(useDiffModalStore.getState().activeIndex).toBe(3)
        const collapse = screen.getByRole("button", { name: "Collapse file list" })
        collapse.focus()
        fireEvent.click(collapse)
        const expand = await screen.findByRole("button", { name: "Expand file list" })
        const content = screen.getByTestId("diff-file-panel-content")
        expect(content).toHaveAttribute("inert")
        expect(content).toHaveAttribute("aria-hidden", "true")
        expect(screen.queryByRole("option")).toBeNull()
        expect(screen.queryByRole("textbox", { name: "Filter files" })).toBeNull()
        expect(expand).toHaveFocus()
        expect(useDiffModalStore.getState().activeIndex).toBe(3)
        fireEvent.click(expand)
        await waitFor(() => {
            expect(screen.getByRole("button", { name: "Collapse file list" })).toHaveAttribute("aria-expanded", "true")
        })
        expect(screen.getByTestId("diff-file-panel-content")).not.toHaveAttribute("aria-hidden")
        expect(screen.getByRole("option", { name: /file3\.ts/ })).toBeInTheDocument()
        expect(screen.getByLabelText("Filter files")).toBeInTheDocument()
        expect(useDiffModalStore.getState().activeIndex).toBe(3)
        expect(screen.getByRole("button", { name: "Collapse file list" })).toHaveAttribute("aria-controls", "diff-file-panel-content")
    })

    it("expands the file panel when a new source replaces a collapsed open source", async () => {
        render(<DiffModal />)
        act(() => {
            useDiffModalStore.getState().openWorktree("/w", [
                { path: "a.ts", origPath: null, status: "M", staged: false }
            ])
        })
        fireEvent.click(screen.getByRole("button", { name: "Collapse file list" }))
        await waitFor(() => {
            expect(screen.getByRole("button", { name: "Expand file list" })).toHaveAttribute("aria-expanded", "false")
        })

        act(() => {
            useDiffModalStore.getState().openWorktree("/w", [
                { path: "b.ts", origPath: null, status: "M", staged: false }
            ])
        })

        await waitFor(() => {
            expect(screen.getByRole("button", { name: "Collapse file list" })).toHaveAttribute("aria-expanded", "true")
        })
        expect(screen.getByTestId("diff-file-panel-content")).not.toHaveAttribute("inert")
        expect(screen.getByTestId("diff-file-panel-content")).not.toHaveAttribute("aria-hidden")
        expect(screen.getByRole("option", { name: /b\.ts/ })).toBeInTheDocument()
    })

    it("reopens with the file list expanded after a collapsed close", async () => {
        render(<DiffModal />)
        act(() => {
            useDiffModalStore.getState().openWorktree("/w", [
                { path: "a.ts", origPath: null, status: "M", staged: false }
            ])
        })
        expect(screen.getByRole("button", { name: "Collapse file list" })).toHaveAttribute("aria-expanded", "true")
        fireEvent.click(screen.getByRole("button", { name: "Collapse file list" }))
        await waitFor(() => {
            expect(screen.getByTestId("diff-file-panel-content")).toHaveAttribute("inert")
        })
        fireEvent.click(screen.getByRole("button", { name: "Close" }))
        expect(useDiffModalStore.getState().open).toBe(false)
        expect(screen.queryByTestId("diff-file-panel-content")).toBeNull()

        act(() => {
            useDiffModalStore.getState().openWorktree("/w", [
                { path: "a.ts", origPath: null, status: "M", staged: false }
            ])
        })

        const content = screen.getByTestId("diff-file-panel-content")
        expect(content).not.toHaveAttribute("inert")
        expect(content).not.toHaveAttribute("aria-hidden")
        expect(screen.getByTestId("diff-files")).toHaveAttribute("data-files-collapsed", "false")
        const toggle = screen.getByRole("button", { name: "Collapse file list" })
        expect(toggle).toHaveAttribute("aria-expanded", "true")
        const handle = screen.getByTestId("diff-files-handle")
        expect(handle).not.toHaveAttribute("disabled")
        expect(handle).not.toHaveAttribute("aria-disabled", "true")
        expect(handle).not.toHaveAttribute("aria-hidden")
        expect(handle.className).not.toMatch(/w-0/)

        fireEvent.click(toggle)
        await waitFor(() => {
            expect(screen.getByRole("button", { name: "Expand file list" })).toHaveAttribute("aria-expanded", "false")
        })
        expect(screen.getByTestId("diff-file-panel-content")).toHaveAttribute("inert")
        expect(screen.getByTestId("diff-file-panel-content")).toHaveAttribute("aria-hidden", "true")
    })

    it("moves focus into the dialog on open and restores it on close", async () => {
        const opener = document.createElement("button")
        opener.textContent = "open-diff"
        document.body.appendChild(opener)
        opener.focus()
        render(<DiffModal />)
        act(() => {
            useDiffModalStore.getState().openWorktree("/w", [
                { path: "a.ts", origPath: null, status: "M", staged: false }
            ])
        })
        await waitFor(() => expect(screen.getByRole("button", { name: "Close" })).toHaveFocus())
        act(() => {
            useDiffModalStore.getState().close()
        })
        await waitFor(() => expect(opener).toHaveFocus())
        opener.remove()
    })

    it("collapses the file handle sliver and keeps the tooltip above the modal", async () => {
        render(<DiffModal />)
        act(() => {
            useDiffModalStore.getState().openWorktree("/w", [
                { path: "a.ts", origPath: null, status: "M", staged: false }
            ])
        })
        const handle = screen.getByTestId("diff-files-handle")
        expect(handle.className).not.toMatch(/w-0/)
        const toggle = screen.getByTestId("diff-files-toggle")
        fireEvent.pointerMove(toggle)
        fireEvent.pointerOver(toggle)
        fireEvent.mouseEnter(toggle)
        fireEvent.focus(toggle)
        const tooltip = await screen.findByTestId("diff-files-toggle-tooltip")
        expect(tooltip.className).toMatch(/z-\[70\]/)
        act(() => {
            (screen.getByTestId("diff-files") as HTMLElement & { __yzPanel?: { collapse: () => void } }).__yzPanel?.collapse()
        })
        await waitFor(() => expect(screen.getByTestId("diff-files-handle")).toHaveAttribute("aria-disabled", "true"))
        expect(screen.getByTestId("diff-files-handle")).toHaveAttribute("aria-hidden", "true")
        expect(screen.getByTestId("diff-files-handle").className).toMatch(/w-0/)
        expect(screen.getByTestId("diff-files-handle").className).toMatch(/pointer-events-none/)
    })
})

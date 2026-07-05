import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"
import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react"

import type { GitStatus } from "@/lib/types"

function makeStatus(over: Partial<GitStatus> = {}): GitStatus {
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

vi.mock("@/lib/ipc", () => ({
    gitStage: vi.fn(async () => undefined),
    gitUnstage: vi.fn(async () => undefined),
    gitDiscard: vi.fn(async () => undefined),
    gitCommit: vi.fn(async () => undefined),
    gitStatus: vi.fn(async () => makeStatus()),
    gitBranches: vi.fn(async () => ({ local: [], remote: [] })),
    // BranchPopover pulls these in through the shared trigger.
    gitCheckout: vi.fn(async () => undefined),
    gitCreateBranch: vi.fn(async () => undefined),
    gitFetch: vi.fn(async () => undefined),
    gitPull: vi.fn(async () => undefined),
    gitPush: vi.fn(async () => undefined),
    logUserAction: vi.fn(async () => undefined)
}))

const ipc = await import("@/lib/ipc")
const { GitNavContent } = await import("@/app/workbench/GitNavContent")
const { useGitStore, initialGitState } = await import("@/state/gitStore")
const { useDiffModalStore } = await import("@/state/diffModalStore")
const { useUiStore, uiInitialState } = await import("@/state/uiStore")

const READY = { status: "ready", root: "/w", version: "2.50.1" } as const

function setReady(status: Partial<GitStatus> = {}) {
    useGitStore.setState({ environment: READY, status: makeStatus(status) })
}

function resetDiffModal() {
    useDiffModalStore.setState({ open: false, source: null, activeIndex: 0, mode: "unified" })
}

describe("GitNavContent — ready state (E1)", () => {
    beforeEach(() => {
        useGitStore.setState(initialGitState)
        useUiStore.setState(uiInitialState)
        resetDiffModal()
    })
    afterEach(() => {
        cleanup()
        vi.clearAllMocks()
    })

    it("missing → guided setup; not-ready → empty state (unchanged)", () => {
        useGitStore.setState({ environment: { status: "missing", reason: "git not found" } })
        const { rerender } = render(<GitNavContent />)
        expect(screen.getByText("未偵測到 Git")).toBeInTheDocument()

        useGitStore.setState({ environment: null })
        rerender(<GitNavContent />)
        expect(screen.getByText("No repository status")).toBeInTheDocument()
    })

    it("renders the commit card: branch pill, ahead/behind, changed pill", () => {
        setReady({
            ahead: 2,
            behind: 1,
            unstaged: [{ path: "b.ts", origPath: null, status: "M" }],
            untracked: ["c.txt"]
        })
        render(<GitNavContent />)
        expect(screen.getByRole("button", { name: "Branches" })).toBeInTheDocument()
        expect(screen.getByText("main")).toBeInTheDocument()
        expect(screen.getByText("↑2")).toBeInTheDocument()
        expect(screen.getByText("↓1")).toBeInTheDocument()
        expect(screen.getByText("2 changed")).toBeInTheDocument()
    })

    it("hides ahead/behind and changed pill when zero", () => {
        setReady()
        render(<GitNavContent />)
        expect(screen.queryByText(/changed$/)).not.toBeInTheDocument()
        expect(screen.queryByText(/^[↑↓]/)).not.toBeInTheDocument()
    })

    it("Commit enabled only with staged files AND a non-empty message", () => {
        setReady({ staged: [{ path: "a.ts", origPath: null, status: "M" }] })
        render(<GitNavContent />)
        const btn = screen.getByRole("button", { name: "Commit" })
        expect(btn).toBeDisabled()

        fireEvent.change(screen.getByPlaceholderText(/commit message/i), {
            target: { value: "feat: x" }
        })
        expect(btn).toBeEnabled()
    })

    it("commit calls gitCommit and clears the shared message", async () => {
        setReady({ staged: [{ path: "a.ts", origPath: null, status: "M" }] })
        render(<GitNavContent />)
        fireEvent.change(screen.getByPlaceholderText(/commit message/i), {
            target: { value: "feat: x" }
        })
        fireEvent.click(screen.getByRole("button", { name: "Commit" }))
        await waitFor(() => expect(ipc.gitCommit).toHaveBeenCalledWith("feat: x"))
        await waitFor(() => expect(useGitStore.getState().commitMessage).toBe(""))
    })

    it("Review diff opens the worktree modal with the flattened files", () => {
        setReady({
            staged: [{ path: "a.ts", origPath: null, status: "M" }],
            unstaged: [{ path: "b.ts", origPath: null, status: "M" }]
        })
        render(<GitNavContent />)
        fireEvent.click(screen.getByRole("button", { name: "Review diff" }))
        const s = useDiffModalStore.getState()
        expect(s.open).toBe(true)
        expect(s.source?.type).toBe("worktree")
        expect(s.source?.type === "worktree" && s.source.files.map((f) => f.path)).toEqual([
            "a.ts",
            "b.ts"
        ])
    })

    it("Review diff is disabled with no changes at all", () => {
        setReady()
        render(<GitNavContent />)
        expect(screen.getByRole("button", { name: "Review diff" })).toBeDisabled()
    })

    // Note: after a runOp the mocked gitStatus refresh empties the list, so each
    // test exercises a single per-file/bulk action (a second click would target
    // a now-removed row). Real refresh returns the true status and rows persist.
    it("renders STAGED and CHANGED lists", () => {
        setReady({
            staged: [{ path: "a.ts", origPath: null, status: "M" }],
            unstaged: [{ path: "b.ts", origPath: null, status: "M" }]
        })
        render(<GitNavContent />)
        expect(screen.getByText("Staged")).toBeInTheDocument()
        expect(screen.getByText("Changed")).toBeInTheDocument()
        expect(screen.getByText("a.ts")).toBeInTheDocument()
        expect(screen.getByText("b.ts")).toBeInTheDocument()
    })

    it("per-file stage forwards the path", async () => {
        setReady({ unstaged: [{ path: "b.ts", origPath: null, status: "M" }] })
        render(<GitNavContent />)
        fireEvent.click(screen.getByRole("button", { name: "Stage b.ts" }))
        await waitFor(() => expect(ipc.gitStage).toHaveBeenCalledWith(["b.ts"]))
    })

    it("per-file unstage forwards the path", async () => {
        setReady({ staged: [{ path: "a.ts", origPath: null, status: "M" }] })
        render(<GitNavContent />)
        fireEvent.click(screen.getByRole("button", { name: "Unstage a.ts" }))
        await waitFor(() => expect(ipc.gitUnstage).toHaveBeenCalledWith(["a.ts"]))
    })

    it("Stage all forwards only the changed buckets", async () => {
        setReady({
            staged: [{ path: "a.ts", origPath: null, status: "M" }],
            unstaged: [{ path: "b.ts", origPath: null, status: "M" }],
            untracked: ["c.txt"]
        })
        render(<GitNavContent />)
        fireEvent.click(screen.getByRole("button", { name: "Stage all" }))
        await waitFor(() => expect(ipc.gitStage).toHaveBeenCalledWith(["b.ts", "c.txt"]))
    })

    it("Unstage all forwards the staged bucket", async () => {
        setReady({
            staged: [
                { path: "a.ts", origPath: null, status: "M" },
                { path: "d.ts", origPath: null, status: "M" }
            ]
        })
        render(<GitNavContent />)
        fireEvent.click(screen.getByRole("button", { name: "Unstage all" }))
        await waitFor(() => expect(ipc.gitUnstage).toHaveBeenCalledWith(["a.ts", "d.ts"]))
    })

    it("clicking a file row opens the modal on that path", () => {
        setReady({
            staged: [{ path: "a.ts", origPath: null, status: "M" }],
            unstaged: [{ path: "b.ts", origPath: null, status: "M" }]
        })
        render(<GitNavContent />)
        fireEvent.click(screen.getByText("b.ts"))
        const s = useDiffModalStore.getState()
        expect(s.open).toBe(true)
        // openWorktree(files, {path:"b.ts", staged:false}) → b.ts is index 1
        // (staged a.ts first).
        expect(s.activeIndex).toBe(1)
    })

    it("staging a file keeps the Local-changes selection following it (T15)", async () => {
        // The panel had this file selected on the unstaged (changes) side.
        useUiStore.getState().selectGitFile("b.ts", false)
        setReady({ unstaged: [{ path: "b.ts", origPath: null, status: "M" }] })
        render(<GitNavContent />)
        fireEvent.click(screen.getByRole("button", { name: "Stage b.ts" }))
        await waitFor(() => expect(ipc.gitStage).toHaveBeenCalledWith(["b.ts"]))
        // Selection follows the row to the staged side so the diff re-resolves.
        await waitFor(() => expect(useUiStore.getState().gitSelectedStaged).toBe(true))
        expect(useUiStore.getState().gitSelectedPath).toBe("b.ts")
    })

    it("unstaging a file keeps the Local-changes selection following it (T15)", async () => {
        useUiStore.getState().selectGitFile("a.ts", true)
        setReady({ staged: [{ path: "a.ts", origPath: null, status: "M" }] })
        render(<GitNavContent />)
        fireEvent.click(screen.getByRole("button", { name: "Unstage a.ts" }))
        await waitFor(() => expect(ipc.gitUnstage).toHaveBeenCalledWith(["a.ts"]))
        await waitFor(() => expect(useUiStore.getState().gitSelectedStaged).toBe(false))
        expect(useUiStore.getState().gitSelectedPath).toBe("a.ts")
    })

    it("clicking the CHANGED row of an MM file opens the UNSTAGED side (F2)", () => {
        // A partially-staged file (M staged AND M unstaged) has two rows with the
        // same path. worktreeFilesFrom lists the staged row first, so a path-only
        // active would always land on the staged side. The CHANGED row must pass
        // { path, staged:false } so it opens on the unstaged (index 1) side.
        setReady({
            staged: [{ path: "mm.ts", origPath: null, status: "M" }],
            unstaged: [{ path: "mm.ts", origPath: null, status: "M" }]
        })
        render(<GitNavContent />)
        // Two "mm.ts" rows render (STAGED + CHANGED); the CHANGED one is the last.
        const rows = screen.getAllByText("mm.ts")
        fireEvent.click(rows[rows.length - 1])
        const s = useDiffModalStore.getState()
        expect(s.open).toBe(true)
        // staged row is index 0, unstaged row is index 1 → CHANGED click lands on 1.
        expect(s.activeIndex).toBe(1)

        // And clicking the STAGED row lands on index 0.
        fireEvent.click(rows[0])
        expect(useDiffModalStore.getState().activeIndex).toBe(0)
    })
})

describe("GitNavContent — hold-to-discard (E1 §1.4)", () => {
    beforeEach(() => {
        vi.useFakeTimers()
        useGitStore.setState(initialGitState)
        resetDiffModal()
    })
    afterEach(() => {
        cleanup()
        vi.clearAllMocks()
        vi.useRealTimers()
    })

    it("holding for the full duration discards all working changes", async () => {
        setReady({
            unstaged: [{ path: "b.ts", origPath: null, status: "M" }],
            untracked: ["c.txt"]
        })
        render(<GitNavContent />)
        const btn = screen.getByRole("button", { name: "Discard all working changes" })
        fireEvent.pointerDown(btn)
        // 760ms hold; the op fires after the timer elapses.
        await vi.advanceTimersByTimeAsync(760)
        expect(ipc.gitDiscard).toHaveBeenCalledWith(["b.ts"], ["c.txt"])
    })

    it("discard all excludes conflicted paths (m7)", async () => {
        setReady({
            unstaged: [{ path: "b.ts", origPath: null, status: "M" }],
            untracked: ["c.txt"],
            conflicted: [{ path: "conf.ts", origPath: null, status: "U" }]
        })
        render(<GitNavContent />)
        const btn = screen.getByRole("button", { name: "Discard all working changes" })
        fireEvent.pointerDown(btn)
        await vi.advanceTimersByTimeAsync(760)
        // Conflicted files must not be restored/cleaned during a merge.
        expect(ipc.gitDiscard).toHaveBeenCalledWith(["b.ts"], ["c.txt"])
        const [tracked, untracked] = (ipc.gitDiscard as ReturnType<typeof vi.fn>).mock.calls[0]
        expect(tracked).not.toContain("conf.ts")
        expect(untracked).not.toContain("conf.ts")
    })

    it("releasing before the threshold does not discard", async () => {
        setReady({ unstaged: [{ path: "b.ts", origPath: null, status: "M" }] })
        render(<GitNavContent />)
        const btn = screen.getByRole("button", { name: "Discard all working changes" })
        fireEvent.pointerDown(btn)
        await vi.advanceTimersByTimeAsync(400)
        fireEvent.pointerUp(btn)
        await vi.advanceTimersByTimeAsync(760)
        expect(ipc.gitDiscard).not.toHaveBeenCalled()
    })

    it("is disabled when there are no working changes", () => {
        setReady({ staged: [{ path: "a.ts", origPath: null, status: "M" }] })
        render(<GitNavContent />)
        expect(
            screen.getByRole("button", { name: "Discard all working changes" })
        ).toBeDisabled()
    })
})

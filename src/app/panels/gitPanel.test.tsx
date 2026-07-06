import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"
import { act, cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react"

import type { GitStatus } from "@/lib/types"

function makeStatus(): GitStatus {
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
        inProgress: null
    }
}

// GitPanel pulls in LocalChangesTab + BranchPopover, both of which import
// @/lib/ipc; mock the whole module (fetch/pull/push are the ones under test,
// the rest back the runOp refresh + child components).
vi.mock("@/lib/ipc", () => ({
    gitFetch: vi.fn(async () => undefined),
    gitPull: vi.fn(async () => undefined),
    gitPush: vi.fn(async () => undefined),
    gitStatus: vi.fn(async () => makeStatus()),
    gitBranches: vi.fn(async () => ({ local: [], remote: [] })),
    gitCheckout: vi.fn(async () => undefined),
    gitCreateBranch: vi.fn(async () => undefined),
    gitStage: vi.fn(async () => undefined),
    gitUnstage: vi.fn(async () => undefined),
    gitDiscard: vi.fn(async () => undefined),
    gitCommit: vi.fn(async () => undefined),
    gitDiffContent: vi.fn(async () => ({
        original: { kind: "full", content: "one\n" },
        modified: { kind: "full", content: "two\n" }
    })),
    gitFileAtRev: vi.fn(async () => ({ kind: "full", content: "x\n" })),
    // Log tab (now the default) mounts and loads the first page.
    gitLogPage: vi.fn(async () => ({ commits: [], hasMore: false })),
    gitLogAuthors: vi.fn(async () => []),
    gitCommitDetail: vi.fn(async () => ({
        subject: "",
        body: "",
        authorName: "",
        authorEmail: "",
        timestamp: 0,
        parents: [],
        files: [],
        totalAdditions: 0,
        totalDeletions: 0
    }))
}))

vi.mock("@/features/logs/userAction", () => ({
    logUserAction: vi.fn(async () => undefined)
}))

vi.mock("@tauri-apps/plugin-clipboard-manager", () => ({
    writeText: vi.fn(async () => undefined)
}))

vi.mock("@tauri-apps/plugin-dialog", () => ({
    confirm: vi.fn(async () => true)
}))

const ipc = await import("@/lib/ipc")
const { GitPanel } = await import("./GitPanel")
const { useGitStore, initialGitState } = await import("@/state/gitStore")
const { useGitLogStore } = await import("@/state/gitLogStore")
const { useUiStore, uiInitialState } = await import("@/state/uiStore")
const { useDiffModalStore } = await import("@/state/diffModalStore")

const ready = { status: "ready", root: "/w", version: "2.50" } as const

describe("GitPanel tab strip", () => {
    beforeEach(() => {
        useGitStore.setState(initialGitState)
        useGitLogStore.getState().reset()
        useUiStore.setState(uiInitialState)
        useDiffModalStore.setState({ open: false, source: null, activeIndex: 0, mode: "unified" })
        vi.clearAllMocks()
    })
    afterEach(() => cleanup())

    it("shows branch pill + Fetch/Pull/Push when ready", () => {
        useGitStore.setState({
            environment: ready,
            status: { ...makeStatus(), branch: "feature/x" }
        })
        render(<GitPanel />)
        expect(screen.getByRole("button", { name: "Branches" })).toBeInTheDocument()
        expect(screen.getByText("feature/x")).toBeInTheDocument()
        expect(screen.getByRole("button", { name: "Fetch" })).toBeInTheDocument()
        expect(screen.getByRole("button", { name: "Pull" })).toBeInTheDocument()
        expect(screen.getByRole("button", { name: "Push" })).toBeInTheDocument()
    })

    it("hides the action cluster when the repo is not ready", () => {
        useGitStore.setState({ environment: { status: "notARepo" } })
        render(<GitPanel />)
        expect(screen.queryByRole("button", { name: "Branches" })).not.toBeInTheDocument()
        expect(screen.queryByRole("button", { name: "Fetch" })).not.toBeInTheDocument()
    })

    it("Fetch / Pull / Push route through runOp", async () => {
        useGitStore.setState({ environment: ready, status: makeStatus() })
        render(<GitPanel />)
        fireEvent.click(screen.getByRole("button", { name: "Fetch" }))
        await waitFor(() => expect(ipc.gitFetch).toHaveBeenCalled())
    })

    it("disables the action buttons while an op is busy", () => {
        useGitStore.setState({ environment: ready, status: makeStatus(), busy: "pull" })
        render(<GitPanel />)
        expect(screen.getByRole("button", { name: "Fetch" })).toBeDisabled()
        expect(screen.getByRole("button", { name: "Pull" })).toBeDisabled()
        expect(screen.getByRole("button", { name: "Push" })).toBeDisabled()
    })

    it("renders the amber changed-count pill on the Local changes tab", () => {
        useGitStore.setState({
            environment: ready,
            status: {
                ...makeStatus(),
                unstaged: [{ path: "a.ts", origPath: null, status: "M" }],
                untracked: ["b.txt"]
            }
        })
        render(<GitPanel />)
        const localTab = screen.getByRole("tab", { name: /Local changes/ })
        expect(localTab).toHaveTextContent("2")
    })

    it("hides the changed-count pill when nothing changed", () => {
        useGitStore.setState({ environment: ready, status: makeStatus() })
        render(<GitPanel />)
        const localTab = screen.getByRole("tab", { name: /Local changes/ })
        expect(localTab).toHaveTextContent(/^Local changes$/)
    })

    it("Console tab is enabled and switches to show console content", async () => {
        useGitStore.setState({
            environment: ready,
            status: makeStatus(),
            consoleLog: [
                { id: 1, cmd: "git fetch", out: ["Done"], tone: "ok", time: "14:01" }
            ]
        })
        render(<GitPanel />)
        const consoleTab = screen.getByRole("tab", { name: "Console" })
        expect(consoleTab).not.toBeDisabled()
        // Radix Tabs activate on pointer-down, not click (see panels.test.tsx).
        fireEvent.mouseDown(consoleTab)
        await waitFor(() => expect(screen.getByText("$ git fetch")).toBeInTheDocument())
        expect(screen.getByText("14:01")).toBeInTheDocument()
    })

    it("defaults to the Log tab (enabled) and loads the first page", async () => {
        useGitStore.setState({ environment: ready, status: makeStatus() })
        render(<GitPanel />)
        const logTab = screen.getByRole("tab", { name: /Log/ })
        expect(logTab).not.toBeDisabled()
        expect(logTab).toHaveAttribute("data-state", "active")
        // LogTab mounted → first page requested.
        await waitFor(() => expect(ipc.gitLogPage).toHaveBeenCalled())
    })

    it("can switch to the Local changes tab", async () => {
        useGitStore.setState({ environment: ready, status: makeStatus() })
        render(<GitPanel />)
        fireEvent.mouseDown(screen.getByRole("tab", { name: /Local changes/ }))
        await waitFor(() =>
            expect(screen.getByText("Select a file to view its diff")).toBeInTheDocument()
        )
    })

    // Seed the log store with a selected commit + loaded detail so the details
    // panel (Compare button + changed-file rows) is wired to the Diff modal.
    // Runs inside act after the mount's first-page load settles so it isn't
    // clobbered by the async loadFirstPage response.
    const commit = {
        hash: "c".repeat(40),
        shortHash: "ccccccc",
        subject: "fix: thing",
        authorName: "Kenji",
        authorEmail: "kenji@yuuzu.dev",
        timestamp: 1_770_000_000,
        parents: ["p".repeat(40)],
        refs: []
    }
    const detail = {
        subject: "fix: thing",
        body: "",
        authorName: "Kenji",
        authorEmail: "kenji@yuuzu.dev",
        timestamp: 1_770_000_000,
        parents: ["p".repeat(40)],
        files: [
            { status: "M", path: "git.rs", oldPath: null, additions: 9, deletions: 3, binary: false }
        ],
        totalAdditions: 9,
        totalDeletions: 3
    }
    const seedSelectedCommit = async () => {
        // Wait for the mount's loadFirstPage to resolve, then seed.
        await waitFor(() => expect(ipc.gitLogPage).toHaveBeenCalled())
        await act(async () => {
            useGitLogStore.setState({
                commits: [commit],
                selectedHash: commit.hash,
                detail,
                detailLoading: false
            })
        })
    }

    it("clicking a changed-file row in Log details opens the Diff modal on that commit", async () => {
        useGitStore.setState({ environment: ready, status: makeStatus() })
        render(<GitPanel />)
        await seedSelectedCommit()
        // The changed-file row is a button labelled by its file name.
        const row = await screen.findByRole("button", { name: /git\.rs/ })
        fireEvent.click(row)
        await waitFor(() => {
            const s = useDiffModalStore.getState()
            expect(s.open).toBe(true)
            expect(s.source?.type).toBe("commit")
        })
    })

    it("Compare is enabled with a loaded detail and opens the commit diff", async () => {
        useGitStore.setState({ environment: ready, status: makeStatus() })
        render(<GitPanel />)
        await seedSelectedCommit()
        const compare = await screen.findByRole("button", { name: "Compare" })
        expect(compare).not.toBeDisabled()
        fireEvent.click(compare)
        await waitFor(() => {
            const s = useDiffModalStore.getState()
            expect(s.open).toBe(true)
            expect(s.source?.type).toBe("commit")
            expect(s.activeIndex).toBe(0)
        })
    })

    it("Compare stays disabled when no commit detail is loaded", () => {
        useGitStore.setState({ environment: ready, status: makeStatus() })
        render(<GitPanel />)
        // No selected commit → details panel shows the empty prompt, no Compare.
        expect(screen.queryByRole("button", { name: "Compare" })).toBeNull()
    })

    it("Compare disables during the select cache-miss window, then re-enables (F3)", async () => {
        // Regression (F3 / T5 Minor #2): selecting a second commit whose detail is
        // not cached clears `detail` (gitLogStore.select) so CommitDetails' existing
        // `!detail` guard disables Compare — closing the stale-flash window where
        // Compare would open hash=B with files=A. When B's detail lands, Compare
        // re-enables and opens the correct commit.
        const commitB = { ...commit, hash: "b".repeat(40), shortHash: "bbbbbbb", subject: "feat: b" }
        const detailB = { ...detail, subject: "feat: b" }
        // B's detail hangs so we can observe the disabled window.
        let releaseB: (d: typeof detailB) => void = () => {}
        ;(ipc.gitCommitDetail as ReturnType<typeof vi.fn>).mockReturnValueOnce(
            new Promise((r) => (releaseB = r))
        )
        useGitStore.setState({ environment: ready, status: makeStatus() })
        render(<GitPanel />)
        await seedSelectedCommit()
        // Baseline: A selected + detail A loaded → Compare enabled.
        expect(await screen.findByRole("button", { name: "Compare" })).not.toBeDisabled()
        // Seed B into the list, then select it (cache miss → detail cleared).
        // Don't await: B's detail hangs (releaseB) so we can observe the window.
        let selecting: Promise<void> = Promise.resolve()
        await act(async () => {
            useGitLogStore.setState({ commits: [commit, commitB] })
            selecting = useGitLogStore.getState().select(commitB.hash)
        })
        // Window: detail null, loading true → Compare disabled.
        expect(useGitLogStore.getState().detail).toBe(null)
        expect(useGitLogStore.getState().detailLoading).toBe(true)
        expect(screen.getByRole("button", { name: "Compare" })).toBeDisabled()
        // B's detail arrives → Compare re-enables and opens the B commit.
        await act(async () => {
            releaseB(detailB)
            await selecting
        })
        const compareB = await screen.findByRole("button", { name: "Compare" })
        expect(compareB).not.toBeDisabled()
        fireEvent.click(compareB)
        await waitFor(() => {
            const s = useDiffModalStore.getState()
            expect(s.open).toBe(true)
            expect(s.source?.type === "commit" && s.source.hash).toBe(commitB.hash)
        })
    })
})

import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"
import { act, cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react"

import type { AuthorEntry, CommitDetail, GitStatus, LogCommit, LogPage } from "@/lib/types"

function makeStatus(overrides: Partial<GitStatus> = {}): GitStatus {
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
        ...overrides
    }
}

// Linear-history commit; parents chain to the next so the graph lays out.
function makeCommit(i: number, overrides: Partial<LogCommit> = {}): LogCommit {
    const hash = `hash${i}`.padEnd(40, "0")
    return {
        hash,
        shortHash: `hash${i}`.slice(0, 7),
        subject: `commit subject ${i}`,
        authorName: i % 2 === 0 ? "Kenji" : "Sora",
        authorEmail: i % 2 === 0 ? "kenji@yuuzu.dev" : "sora@yuuzu.dev",
        timestamp: 1_770_000_000 - i * 3600,
        parents: [`hash${i + 1}`.padEnd(40, "0")],
        refs: [],
        ...overrides
    }
}

function makeDetail(overrides: Partial<CommitDetail> = {}): CommitDetail {
    return {
        subject: "commit subject 0",
        body: "",
        authorName: "Kenji",
        authorEmail: "kenji@yuuzu.dev",
        timestamp: 1_770_000_000,
        parents: ["hash1".padEnd(40, "0")],
        files: [
            { status: "M", path: "git.rs", oldPath: null, additions: 9, deletions: 3, binary: false }
        ],
        totalAdditions: 9,
        totalDeletions: 3,
        ...overrides
    }
}

// Controllable ipc mocks. gitLogPage / gitCommitDetail are reassigned per test.
const gitLogPage = vi.fn<(...a: unknown[]) => Promise<LogPage>>(async () => ({
    commits: [],
    hasMore: false
}))
const gitLogAuthors = vi.fn<() => Promise<AuthorEntry[]>>(async () => [])
const gitCommitDetail = vi.fn<(hash: string) => Promise<CommitDetail>>(async () => makeDetail())
const gitCheckout = vi.fn(async () => undefined)
const logUserAction = vi.fn(async () => undefined)
const writeText = vi.fn(async () => undefined)

vi.mock("@/lib/ipc", () => ({
    gitLogPage: (...a: unknown[]) => gitLogPage(...a),
    gitLogAuthors: () => gitLogAuthors(),
    gitCommitDetail: (h: string) => gitCommitDetail(h),
    gitCheckout: () => gitCheckout(),
    logUserAction: () => logUserAction(),
    // gitStore also imports these; harmless stubs for its module graph.
    gitDetect: vi.fn(async () => ({ status: "ready", root: "/w", version: "2.50" })),
    gitStatus: vi.fn(async () => makeStatus()),
    gitBranches: vi.fn(async () => ({ local: [], remote: [] })),
    gitRemoteProbe: vi.fn(async () => "no"),
    gitFetch: vi.fn(async () => undefined),
    gitPull: vi.fn(async () => undefined),
    gitPush: vi.fn(async () => undefined),
    gitCreateBranch: vi.fn(async () => undefined)
}))

vi.mock("@tauri-apps/plugin-clipboard-manager", () => ({
    writeText: () => writeText()
}))

const { LogTab } = await import("./LogTab")
const { useGitLogStore } = await import("@/state/gitLogStore")
const { useGitStore, initialGitState } = await import("@/state/gitStore")
const { useWorkspaceStore } = await import("@/state/workspaceStore")

async function renderLog() {
    render(<LogTab />)
    // Let the mount effect's loadFirstPage resolve.
    await act(async () => {
        await Promise.resolve()
    })
}

beforeEach(() => {
    useGitLogStore.getState().reset()
    useGitStore.setState({ ...initialGitState, status: makeStatus() })
    useWorkspaceStore.setState({ groups: [{ tabs: [], activePath: null }] })
    gitLogPage.mockReset().mockResolvedValue({ commits: [], hasMore: false })
    gitLogAuthors.mockReset().mockResolvedValue([])
    gitCommitDetail.mockReset().mockResolvedValue(makeDetail())
    gitCheckout.mockClear()
    logUserAction.mockClear()
    writeText.mockClear()
})

afterEach(() => cleanup())

describe("LogTab commit list", () => {
    it("renders commit rows (subject / author / date) and a ref chip", async () => {
        gitLogPage.mockResolvedValue({
            commits: [
                makeCommit(0, { subject: "Merge feature", refs: [{ name: "main", kind: "local" }] }),
                makeCommit(1, { subject: "second commit" })
            ],
            hasMore: false
        })
        await renderLog()

        expect(await screen.findByText("Merge feature")).toBeInTheDocument()
        expect(screen.getByText("second commit")).toBeInTheDocument()
        expect(screen.getAllByText("Kenji").length).toBeGreaterThan(0)
        expect(screen.getByText("Sora")).toBeInTheDocument()
        // ref chip
        expect(screen.getByText("main")).toBeInTheDocument()
    })

    it("selecting a row loads and shows the commit details", async () => {
        gitLogPage.mockResolvedValue({ commits: [makeCommit(0)], hasMore: false })
        gitCommitDetail.mockResolvedValue(makeDetail({ subject: "commit subject 0" }))
        await renderLog()

        fireEvent.click(await screen.findByText("commit subject 0"))
        await waitFor(() => expect(gitCommitDetail).toHaveBeenCalled())
        // details panel hash chip + changed file
        expect(await screen.findByText("git.rs")).toBeInTheDocument()
        expect(screen.getByText("kenji@yuuzu.dev")).toBeInTheDocument()
    })

    it("shows the empty state when no commits match", async () => {
        gitLogPage.mockResolvedValue({ commits: [], hasMore: false })
        await renderLog()
        expect(
            await screen.findByText("No commits match the current filters.")
        ).toBeInTheDocument()
    })

    it("shows an error with a Retry button that reloads", async () => {
        gitLogPage.mockRejectedValueOnce(new Error("boom"))
        await renderLog()
        const retry = await screen.findByRole("button", { name: "Retry" })
        gitLogPage.mockResolvedValue({ commits: [makeCommit(0)], hasMore: false })
        fireEvent.click(retry)
        expect(await screen.findByText("commit subject 0")).toBeInTheDocument()
    })

    it("reloads the first page when HEAD moves (headOid changes)", async () => {
        gitLogPage.mockResolvedValue({ commits: [makeCommit(0)], hasMore: false })
        await renderLog()
        await screen.findByText("commit subject 0")
        gitLogPage.mockClear()
        // Simulate a commit / checkout moving HEAD.
        await act(async () => {
            useGitStore.setState({ status: makeStatus({ headOid: "1".repeat(40) }) })
            await Promise.resolve()
        })
        await waitFor(() => expect(gitLogPage).toHaveBeenCalled())
    })
})

describe("LogTab filters", () => {
    it("debounces the search input then calls setFilters", async () => {
        vi.useFakeTimers()
        try {
            const setFilters = vi.spyOn(useGitLogStore.getState(), "setFilters")
            render(<LogTab />)
            const input = screen.getByLabelText("Filter commits")
            fireEvent.change(input, { target: { value: "fix" } })
            // not yet — inside the 250ms window
            expect(setFilters).not.toHaveBeenCalled()
            act(() => {
                vi.advanceTimersByTime(250)
            })
            expect(setFilters).toHaveBeenCalledWith({ query: "fix" })
        } finally {
            vi.useRealTimers()
        }
    })

    it("unmount clears the pending debounce so a trailing timer never fires setFilters (T5-1)", () => {
        // Restore any spies leaked by sibling tests (they spy on the store's
        // setFilters without restoring), so our fresh spy sees only our calls.
        vi.restoreAllMocks()
        // Start from a pristine fake-timer clock: a sibling test may have advanced
        // it, which would otherwise make our setTimeout fire immediately.
        vi.useRealTimers()
        vi.useFakeTimers({ now: 0 })
        // Reset filters so we can assert they never pick up the typed query.
        useGitLogStore.setState({ filters: { query: "", author: null, since: null, until: null } })
        try {
            let unmount: () => void = () => {}
            act(() => {
                unmount = render(<LogTab />).unmount
            })
            // Flush + clear LogTab's mount-effect timers so only the debounce timer
            // we set below is in flight.
            act(() => {
                vi.clearAllTimers()
            })
            const input = screen.getByLabelText("Filter commits")
            fireEvent.change(input, { target: { value: "fix" } })
            // Unmount inside the 250ms window (simulates switching tabs away).
            act(() => {
                unmount()
            })
            // Advancing past the debounce must NOT fire setFilters — the cleanup
            // cleared the timer. Without it the trailing timer would push the typed
            // query into the store after the input is gone (filters/input desync).
            act(() => {
                vi.advanceTimersByTime(250)
            })
            expect(useGitLogStore.getState().filters.query).toBe("")
        } finally {
            vi.useRealTimers()
        }
    })

    it("User dropdown lists authors and filters by name", async () => {
        gitLogAuthors.mockResolvedValue([
            { name: "Kenji", email: "kenji@yuuzu.dev" },
            { name: "Sora", email: "sora@yuuzu.dev" }
        ])
        await renderLog()
        // authors load fire-and-forget; wait for them in state
        await waitFor(() => expect(useGitLogStore.getState().authors.length).toBe(2))

        fireEvent.click(screen.getByRole("button", { name: "User filter" }))
        const menu = screen.getByRole("menu")
        fireEvent.click(within(menu).getByRole("menuitem", { name: "Sora" }))
        expect(useGitLogStore.getState().filters.author).toBe("Sora")
    })

    it("Date dropdown maps labels to filters.since", async () => {
        await renderLog()
        fireEvent.click(screen.getByRole("button", { name: "Date filter" }))
        fireEvent.click(screen.getByRole("menuitem", { name: "Last 7 days" }))
        expect(useGitLogStore.getState().filters.since).toBe("7 days ago")

        fireEvent.click(screen.getByRole("button", { name: "Date filter" }))
        fireEvent.click(screen.getByRole("menuitem", { name: "Last 30 days" }))
        expect(useGitLogStore.getState().filters.since).toBe("30 days ago")

        fireEvent.click(screen.getByRole("button", { name: "Date filter" }))
        fireEvent.click(screen.getByRole("menuitem", { name: "All" }))
        expect(useGitLogStore.getState().filters.since).toBe(null)
    })
})

describe("LogTab infinite scroll + virtualization", () => {
    it("triggers loadMore when scrolled near the bottom", async () => {
        gitLogPage.mockResolvedValue({
            commits: Array.from({ length: 200 }, (_, i) => makeCommit(i)),
            hasMore: true
        })
        await renderLog()
        await screen.findByText("commit subject 0")
        gitLogPage.mockClear()
        gitLogPage.mockResolvedValue({
            commits: Array.from({ length: 50 }, (_, i) => makeCommit(200 + i)),
            hasMore: false
        })

        const scroll = screen.getByTestId("log-scroll")
        // Simulate a near-bottom scroll: total 200*32 = 6400px.
        Object.defineProperty(scroll, "scrollHeight", { value: 6400, configurable: true })
        Object.defineProperty(scroll, "clientHeight", { value: 600, configurable: true })
        Object.defineProperty(scroll, "scrollTop", { value: 5600, configurable: true })
        fireEvent.scroll(scroll)

        await waitFor(() => expect(gitLogPage).toHaveBeenCalled())
    })

    it("virtualizes: 1000 commits render only a windowed subset of rows", async () => {
        gitLogPage.mockResolvedValue({
            commits: Array.from({ length: 1000 }, (_, i) => makeCommit(i)),
            hasMore: false
        })
        await renderLog()
        await screen.findByText("commit subject 0")
        // With a ~600px viewport and 32px rows + overscan, far fewer than 1000
        // commit subjects should be in the DOM.
        const rendered = screen.getAllByText(/^commit subject \d+$/)
        expect(rendered.length).toBeLessThan(200)
        expect(rendered.length).toBeGreaterThan(0)
    })
})

describe("LogTab details actions", () => {
    async function renderWithSelection() {
        gitLogPage.mockResolvedValue({ commits: [makeCommit(0)], hasMore: false })
        await renderLog()
        fireEvent.click(await screen.findByText("commit subject 0"))
        await screen.findByText("git.rs")
    }

    it("copies the full hash", async () => {
        await renderWithSelection()
        fireEvent.click(screen.getByRole("button", { name: "Copy hash" }))
        await waitFor(() => expect(writeText).toHaveBeenCalled())
    })

    it("Checkout routes through gitStore.runOp / gitCheckout", async () => {
        await renderWithSelection()
        fireEvent.click(screen.getByRole("button", { name: "Checkout" }))
        await waitFor(() => expect(gitCheckout).toHaveBeenCalled())
    })

    it("blocks Checkout while an editor tab is dirty", async () => {
        useWorkspaceStore.setState({
            groups: [
                {
                    tabs: [
                        { path: "a.ts", name: "a.ts", dirty: true, externallyModified: false }
                    ],
                    activePath: "a.ts"
                }
            ]
        })
        await renderWithSelection()
        fireEvent.click(screen.getByRole("button", { name: "Checkout" }))
        expect(gitCheckout).not.toHaveBeenCalled()
        expect(await screen.findByText(/未儲存的變更/)).toBeInTheDocument()
    })

    it("Compare / Cherry-pick / Reset are present and disabled", async () => {
        await renderWithSelection()
        expect(screen.getByRole("button", { name: "Compare" })).toBeDisabled()
        expect(screen.getByRole("button", { name: "Cherry-pick" })).toBeDisabled()
        expect(screen.getByRole("button", { name: "Reset main to here…" })).toBeDisabled()
    })

    it("shows the empty details prompt before any selection", async () => {
        gitLogPage.mockResolvedValue({ commits: [makeCommit(0)], hasMore: false })
        await renderLog()
        expect(
            await screen.findByText("Select a commit to view details")
        ).toBeInTheDocument()
    })
})

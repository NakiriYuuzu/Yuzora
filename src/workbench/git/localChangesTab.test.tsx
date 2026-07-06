import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"
import { fireEvent, render, screen, waitFor, within } from "@testing-library/react"

import type { GitStatus } from "../../lib/types"

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

vi.mock("../../lib/ipc", () => ({
    gitStage: vi.fn(async () => undefined),
    gitUnstage: vi.fn(async () => undefined),
    gitStatus: vi.fn(async () => makeStatus()),
    gitBranches: vi.fn(async () => ({ local: [], remote: [] })),
    gitFetch: vi.fn(async () => undefined),
    gitRemoteProbe: vi.fn(async () => "no"),
    gitDetect: vi.fn(async () => ({ status: "ready", root: "/w", version: "2.50.1" })),
    gitDiffContent: vi.fn(async () => ({
        original: { kind: "full", content: "one\n" },
        modified: { kind: "full", content: "two\n" }
    }))
}))

vi.mock("@/features/logs/userAction", () => ({
    logUserAction: vi.fn(async () => undefined)
}))

// Imported after the mocks so the component and tests share the mocked modules.
const ipc = await import("../../lib/ipc")
const { LocalChangesTab } = await import("./LocalChangesTab")
const { useGitStore, initialGitState } = await import("../../state/gitStore")
const { useUiStore, uiInitialState } = await import("../../state/uiStore")

describe("LocalChangesTab", () => {
    beforeEach(() => {
        useGitStore.setState(initialGitState)
        useUiStore.setState(uiInitialState)
    })
    afterEach(() => {
        vi.clearAllMocks()
    })

    it("renders a single flat list with staged files first", () => {
        useGitStore.setState({
            status: {
                ...makeStatus(),
                staged: [{ path: "z.ts", origPath: null, status: "M" }],
                unstaged: [{ path: "b.ts", origPath: null, status: "M" }],
                untracked: ["c.txt"]
            }
        })
        render(<LocalChangesTab />)
        // Header label present; staged/changes section headers gone.
        expect(screen.getByText("Local changes")).toBeInTheDocument()
        expect(screen.queryByText(/^Staged \d/)).not.toBeInTheDocument()
        expect(screen.queryByText(/^Changes \d/)).not.toBeInTheDocument()

        // All three files render; staged z.ts sorts before the changed files.
        const names = screen.getAllByText(/\.(ts|txt)$/).map((n) => n.textContent)
        expect(names).toEqual(["z.ts", "b.ts", "c.txt"])
    })

    it("has no commit box (moved to the sidebar)", () => {
        useGitStore.setState({
            status: { ...makeStatus(), staged: [{ path: "a.ts", origPath: null, status: "M" }] }
        })
        render(<LocalChangesTab />)
        expect(screen.queryByPlaceholderText(/commit message/i)).not.toBeInTheDocument()
        expect(screen.queryByRole("button", { name: "Commit" })).not.toBeInTheDocument()
    })

    it("stage button forwards path and refreshes via runOp", async () => {
        useGitStore.setState({
            status: { ...makeStatus(), unstaged: [{ path: "b.ts", origPath: null, status: "M" }] }
        })
        render(<LocalChangesTab />)
        fireEvent.click(screen.getByRole("button", { name: "Stage b.ts" }))
        await waitFor(() => expect(ipc.gitStage).toHaveBeenCalledWith(["b.ts"]))
    })

    it("staged row unstage button forwards path via runOp", async () => {
        useGitStore.setState({
            status: { ...makeStatus(), staged: [{ path: "a.ts", origPath: null, status: "M" }] }
        })
        render(<LocalChangesTab />)
        fireEvent.click(screen.getByRole("button", { name: "Unstage a.ts" }))
        await waitFor(() => expect(ipc.gitUnstage).toHaveBeenCalledWith(["a.ts"]))
    })

    it("Stage all forwards only the changed paths", async () => {
        useGitStore.setState({
            status: {
                ...makeStatus(),
                staged: [{ path: "a.ts", origPath: null, status: "M" }],
                unstaged: [{ path: "b.ts", origPath: null, status: "M" }],
                untracked: ["c.txt"]
            }
        })
        render(<LocalChangesTab />)
        fireEvent.click(screen.getByRole("button", { name: "Stage all" }))
        await waitFor(() => expect(ipc.gitStage).toHaveBeenCalledWith(["b.ts", "c.txt"]))
    })

    it("row click loads diff for the selected side", async () => {
        useGitStore.setState({
            status: { ...makeStatus(), staged: [{ path: "a.ts", origPath: null, status: "M" }] }
        })
        render(<LocalChangesTab />)
        fireEvent.click(screen.getByText("a.ts"))
        await waitFor(() => expect(ipc.gitDiffContent).toHaveBeenCalledWith("a.ts", true))
    })

    it("diff header shows the selected file's status badge (§2.5)", async () => {
        useGitStore.setState({
            status: { ...makeStatus(), staged: [{ path: "a.ts", origPath: null, status: "M" }] }
        })
        render(<LocalChangesTab />)
        fireEvent.click(screen.getByText("a.ts"))
        // Scope to the diff header (the container that holds the Unified toggle),
        // so the assertion targets the header badge, not the file row's badge.
        const header = screen.getByRole("button", { name: "Unified" }).closest("[data-diff-header]")
        expect(header).not.toBeNull()
        await waitFor(() =>
            expect(within(header as HTMLElement).getByText("M")).toBeInTheDocument()
        )
    })

    it("diff header shows language label + +N/−N line stats (§2.5)", async () => {
        // Mocked gitDiffContent → original "one\n", modified "two\n": 1 add, 1 del.
        useGitStore.setState({
            status: { ...makeStatus(), staged: [{ path: "a.ts", origPath: null, status: "M" }] }
        })
        render(<LocalChangesTab />)
        fireEvent.click(screen.getByText("a.ts"))
        const header = screen.getByRole("button", { name: "Unified" }).closest("[data-diff-header]")
        const h = header as HTMLElement
        await waitFor(() => expect(within(h).getByText("+1")).toBeInTheDocument())
        expect(within(h).getByText("−1")).toBeInTheDocument()
        expect(within(h).getByText("TypeScript")).toBeInTheDocument()
    })

    it("no stats before a file is selected", () => {
        useGitStore.setState({
            status: { ...makeStatus(), unstaged: [{ path: "b.ts", origPath: null, status: "M" }] }
        })
        render(<LocalChangesTab />)
        const header = screen.getByRole("button", { name: "Unified" }).closest("[data-diff-header]")
        const h = header as HTMLElement
        expect(within(h).queryByText(/^[+−]\d/)).not.toBeInTheDocument()
    })
})

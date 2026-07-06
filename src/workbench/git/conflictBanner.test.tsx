import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"
import { fireEvent, render, screen, waitFor } from "@testing-library/react"

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
    gitConflictAbort: vi.fn(async () => undefined),
    gitConflictContinue: vi.fn(async () => undefined),
    gitStatus: vi.fn(async () => makeStatus()),
    gitBranches: vi.fn(async () => ({ local: [], remote: [] })),
    gitFetch: vi.fn(async () => undefined),
    gitRemoteProbe: vi.fn(async () => "no"),
    gitDetect: vi.fn(async () => ({ status: "ready", root: "/w", version: "2.50.1" }))
}))

vi.mock("@/features/logs/userAction", () => ({
    logUserAction: vi.fn(async () => undefined)
}))

vi.mock("@tauri-apps/plugin-dialog", () => ({
    confirm: vi.fn(async () => true)
}))

// Imported after the mocks so the component and tests share the mocked modules.
const ipc = await import("../../lib/ipc")
const dialog = await import("@tauri-apps/plugin-dialog")
const { ConflictBanner } = await import("./ConflictBanner")
const { useGitStore, initialGitState } = await import("../../state/gitStore")
const { useUiStore, uiInitialState } = await import("../../state/uiStore")

describe("ConflictBanner", () => {
    beforeEach(() => {
        useGitStore.setState(initialGitState)
        useUiStore.setState(uiInitialState)
    })
    afterEach(() => {
        vi.clearAllMocks()
    })

    it("renders the in-progress op and conflicted file list", () => {
        useGitStore.setState({
            status: {
                ...makeStatus(),
                inProgress: "merge",
                conflicted: [
                    { path: "a.ts", origPath: null, status: "UU" },
                    { path: "b.ts", origPath: null, status: "UU" }
                ]
            }
        })
        render(<ConflictBanner />)
        expect(screen.getByText(/merge/i)).toBeInTheDocument()
        expect(screen.getByText("a.ts")).toBeInTheDocument()
        expect(screen.getByText("b.ts")).toBeInTheDocument()
    })

    it("renders nothing when no op is in progress", () => {
        useGitStore.setState({ status: { ...makeStatus(), inProgress: null } })
        const { container } = render(<ConflictBanner />)
        expect(container).toBeEmptyDOMElement()
    })

    it("clicking a file selects it via selectGitFile", () => {
        useGitStore.setState({
            status: {
                ...makeStatus(),
                inProgress: "merge",
                conflicted: [{ path: "a.ts", origPath: null, status: "UU" }]
            }
        })
        render(<ConflictBanner />)
        fireEvent.click(screen.getByText("a.ts"))
        expect(useUiStore.getState().gitSelectedPath).toBe("a.ts")
        expect(useUiStore.getState().gitSelectedStaged).toBe(false)
    })

    it("Abort calls gitConflictAbort after confirm", async () => {
        useGitStore.setState({ status: { ...makeStatus(), inProgress: "merge" } })
        render(<ConflictBanner />)
        fireEvent.click(screen.getByRole("button", { name: "Abort" }))
        await waitFor(() => expect(dialog.confirm).toHaveBeenCalled())
        await waitFor(() => expect(ipc.gitConflictAbort).toHaveBeenCalledWith("merge"))
    })

    it("Abort records a cherry-pick-specific console label", async () => {
        useGitStore.setState({ status: { ...makeStatus(), inProgress: "cherry-pick" } })
        render(<ConflictBanner />)
        fireEvent.click(screen.getByRole("button", { name: "Abort" }))
        await waitFor(() => expect(ipc.gitConflictAbort).toHaveBeenCalledWith("cherry-pick"))
        await waitFor(() =>
            expect(useGitStore.getState().consoleLog[0]?.cmd).toBe("git cherry-pick --abort")
        )
    })

    it("Abort does nothing when confirm is declined", async () => {
        vi.mocked(dialog.confirm).mockResolvedValueOnce(false)
        useGitStore.setState({ status: { ...makeStatus(), inProgress: "merge" } })
        render(<ConflictBanner />)
        fireEvent.click(screen.getByRole("button", { name: "Abort" }))
        await waitFor(() => expect(dialog.confirm).toHaveBeenCalled())
        expect(ipc.gitConflictAbort).not.toHaveBeenCalled()
    })

    it("Continue calls gitConflictContinue after confirm", async () => {
        useGitStore.setState({ status: { ...makeStatus(), inProgress: "rebase" } })
        render(<ConflictBanner />)
        fireEvent.click(screen.getByRole("button", { name: "Continue" }))
        await waitFor(() => expect(ipc.gitConflictContinue).toHaveBeenCalledWith("rebase"))
    })

    it("shows lastError below the banner when continue fails", () => {
        useGitStore.setState({
            status: { ...makeStatus(), inProgress: "merge" },
            lastError: "error: Committing is not possible because you have unmerged files."
        })
        render(<ConflictBanner />)
        expect(screen.getByText(/unmerged files/i)).toBeInTheDocument()
    })
})

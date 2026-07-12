import { act, cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react"
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"

import type { GitStatus } from "@/lib/types"

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

let refreshedStatus = makeStatus()
const ipcMocks = vi.hoisted(() => ({
    gitRollbackPaths: vi.fn(),
    gitStatus: vi.fn()
}))
const { gitRollbackPaths, gitStatus } = ipcMocks

vi.mock("@/lib/ipc", () => ({
    gitRollbackPaths: ipcMocks.gitRollbackPaths,
    gitStatus: ipcMocks.gitStatus,
    gitBranches: vi.fn(async () => ({ local: [], remote: [] })),
    gitDetect: vi.fn(async () => ({ status: "ready", root: "/w", version: "2.50.1" })),
    gitFetch: vi.fn(async () => undefined),
    gitRemoteProbe: vi.fn(async () => "no")
}))

const { GitRollbackDialog } = await import("./GitRollbackDialog")
const { gitChangeRows } = await import("./gitChangeSelection")
const { useGitRollbackDialogStore } = await import("@/state/gitRollbackDialogStore")
const { initialGitState, useGitStore } = await import("@/state/gitStore")
const { uiInitialState, useUiStore } = await import("@/state/uiStore")
const { useWorkspaceStore } = await import("@/state/workspaceStore")
const i18n = (await import("@/lib/i18n")).default

const READY = { status: "ready", root: "/w", version: "2.50.1" } as const

function openRollback(status: GitStatus, targets = gitChangeRows(status)) {
    refreshedStatus = status
    useGitStore.setState({ environment: READY, status })
    render(<GitRollbackDialog />)
    return useGitRollbackDialogStore.getState().request({
        repositoryRoot: "/w",
        targets
    })
}

beforeEach(async () => {
    await i18n.changeLanguage("en")
    refreshedStatus = makeStatus()
    gitRollbackPaths.mockReset()
    gitRollbackPaths.mockResolvedValue({ restored: [], preservedUntracked: [], deleted: [] })
    gitStatus.mockReset()
    gitStatus.mockImplementation(async () => refreshedStatus)
    useGitStore.setState(initialGitState)
    useUiStore.setState(uiInitialState)
    useWorkspaceStore.setState({
        workspacePath: "/w",
        groups: [{ tabs: [], activePath: null }],
        activeGroupIndex: 0
    })
    useGitRollbackDialogStore.setState({ pending: null })
})

afterEach(() => {
    const pending = useGitRollbackDialogStore.getState().pending
    if (pending) useGitRollbackDialogStore.getState().respond(false)
    cleanup()
    vi.clearAllMocks()
})

describe("GitRollbackDialog", () => {
    it("dedupes partially-staged paths, shows classifications, and defaults deletion OFF", async () => {
        const status = makeStatus({
            staged: [{ path: "partial.ts", origPath: null, status: "A" }],
            unstaged: [{ path: "partial.ts", origPath: null, status: "M" }],
            untracked: ["scratch.txt"]
        })
        const decision = openRollback(status)

        expect(await screen.findByText("Rollback selected changes")).toBeInTheDocument()
        expect(screen.getAllByText("partial.ts")).toHaveLength(1)
        expect(screen.getByText("Added")).toBeInTheDocument()
        expect(screen.getByText("Untracked")).toBeInTheDocument()
        const checkbox = screen.getByRole("checkbox", {
            name: "Also delete untracked / newly added files"
        })
        expect(checkbox).not.toBeChecked()

        fireEvent.click(screen.getByRole("button", { name: "Rollback" }))
        await waitFor(() => expect(gitRollbackPaths).toHaveBeenCalledTimes(1))
        expect(gitRollbackPaths.mock.calls[0]?.[0]).toBe("/w")
        expect(gitRollbackPaths.mock.calls[0]?.[1]).toHaveLength(2)
        expect(gitRollbackPaths.mock.calls[0]?.[2]).toBe(false)
        await expect(decision).resolves.toBe(true)
    })

    it("explicit checkbox ON is the only path that requests deletion and closes deleted clean tabs", async () => {
        const status = makeStatus({ untracked: ["scratch.txt"] })
        useWorkspaceStore.setState({
            groups: [{
                tabs: [{
                    path: "/w/scratch.txt",
                    name: "scratch.txt",
                    dirty: false,
                    externallyModified: false
                }],
                activePath: "/w/scratch.txt"
            }]
        })
        gitRollbackPaths.mockResolvedValue({
            restored: [],
            preservedUntracked: [],
            deleted: ["scratch.txt"]
        })
        const decision = openRollback(status)
        refreshedStatus = makeStatus()

        fireEvent.click(await screen.findByRole("checkbox"))
        fireEvent.click(screen.getByRole("button", { name: "Rollback" }))
        await expect(decision).resolves.toBe(true)
        expect(gitRollbackPaths.mock.calls[0]?.[0]).toBe("/w")
        expect(gitRollbackPaths.mock.calls[0]?.[2]).toBe(true)
        expect(useWorkspaceStore.getState().groups[0].tabs).toEqual([])
    })

    it("conflict and dirty editor gates disable confirm with actionable reasons", async () => {
        const conflict = makeStatus({
            conflicted: [{ path: "merge.ts", origPath: null, status: "UU" }]
        })
        const conflictDecision = openRollback(conflict)
        expect(await screen.findByText("Complete or abort the conflict operation first.")).toBeInTheDocument()
        expect(screen.getByRole("button", { name: "Rollback" })).toBeDisabled()
        useGitRollbackDialogStore.getState().respond(false)
        await expect(conflictDecision).resolves.toBe(false)
        cleanup()

        const dirtyStatus = makeStatus({
            unstaged: [{ path: "dirty.ts", origPath: null, status: "M" }]
        })
        useWorkspaceStore.setState({
            groups: [{
                tabs: [{
                    path: "/w/dirty.ts",
                    name: "dirty.ts",
                    dirty: true,
                    externallyModified: false
                }],
                activePath: "/w/dirty.ts"
            }]
        })
        openRollback(dirtyStatus)
        expect(await screen.findByText("Save or close unsaved content first.")).toBeInTheDocument()
        expect(screen.getByRole("button", { name: "Rollback" })).toBeDisabled()
        expect(gitRollbackPaths).not.toHaveBeenCalled()
    })

    it("shows the current operation if another Git action becomes busy while open", async () => {
        openRollback(makeStatus({ untracked: ["scratch.txt"] }))
        await screen.findByText("Rollback selected changes")
        act(() => useGitStore.setState({ busy: "pull" }))
        expect(await screen.findByRole("alert")).toHaveTextContent(
            "Git operation 'pull' is in progress."
        )
        expect(screen.getByRole("button", { name: "Rollback" })).toBeDisabled()
    })

    it("treats a legacy untracked directory row as affecting dirty descendant tabs", async () => {
        const status = makeStatus({ untracked: ["scratch/"] })
        useWorkspaceStore.setState({
            groups: [{
                tabs: [{
                    path: "/w/scratch/sub/dirty.ts",
                    name: "dirty.ts",
                    dirty: true,
                    externallyModified: false
                }],
                activePath: "/w/scratch/sub/dirty.ts"
            }]
        })
        openRollback(status)

        expect(await screen.findByText("Save or close unsaved content first.")).toBeInTheDocument()
        expect(screen.getByRole("button", { name: "Rollback" })).toBeDisabled()
        expect(gitRollbackPaths).not.toHaveBeenCalled()
    })

    it("closes clean descendant tabs if a backend reports a deleted directory path", async () => {
        const status = makeStatus({ untracked: ["scratch/"] })
        useWorkspaceStore.setState({
            groups: [{
                tabs: [{
                    path: "/w/scratch/sub/clean.ts",
                    name: "clean.ts",
                    dirty: false,
                    externallyModified: false
                }],
                activePath: "/w/scratch/sub/clean.ts"
            }]
        })
        gitRollbackPaths.mockResolvedValue({
            restored: [],
            preservedUntracked: [],
            deleted: ["scratch/"]
        })
        const decision = openRollback(status)
        refreshedStatus = makeStatus()

        fireEvent.click(await screen.findByRole("checkbox"))
        fireEvent.click(screen.getByRole("button", { name: "Rollback" }))
        await expect(decision).resolves.toBe(true)
        expect(useWorkspaceStore.getState().groups[0].tabs).toEqual([])
    })

    it("rechecks dirty buffers at confirm time even if the rendered button was stale", async () => {
        const status = makeStatus({
            unstaged: [{ path: "race.ts", origPath: null, status: "M" }]
        })
        openRollback(status)
        const confirm = await screen.findByRole("button", { name: "Rollback" })
        expect(confirm).toBeEnabled()

        act(() => {
            useWorkspaceStore.setState({
                groups: [{
                    tabs: [{
                        path: "/w/race.ts",
                        name: "race.ts",
                        dirty: true,
                        externallyModified: false
                    }],
                    activePath: "/w/race.ts"
                }]
            })
        })
        // Force the click through to exercise the handler's second preflight,
        // independent from the render-time disabled attribute.
        confirm.removeAttribute("disabled")
        fireEvent.click(confirm)
        expect(await screen.findByRole("alert")).toHaveTextContent(
            "Save or close unsaved content first."
        )
        expect(gitRollbackPaths).not.toHaveBeenCalled()
    })

    it("rejects a repository switch before invoking destructive IPC", async () => {
        const status = makeStatus({ untracked: ["same-name.ts"] })
        openRollback(status)
        const confirm = await screen.findByRole("button", { name: "Rollback" })
        act(() => useGitStore.setState({
            environment: { status: "ready", root: "/other", version: "2.50.1" },
            status
        }))
        fireEvent.click(confirm)

        expect(await screen.findByRole("alert")).toHaveTextContent(
            "The selected changes are out of date. Open the menu again."
        )
        expect(gitRollbackPaths).not.toHaveBeenCalled()
    })

    it("backend error stays open, refreshes possible partial effects, and reconciles stale selection", async () => {
        const status = makeStatus({
            unstaged: [{ path: "partial-failure.ts", origPath: null, status: "M" }]
        })
        const rows = gitChangeRows(status)
        useUiStore.getState().selectGitChange(rows[0], rows, "single")
        gitRollbackPaths.mockRejectedValue(new Error("second step failed"))
        openRollback(status, rows)
        refreshedStatus = makeStatus()

        fireEvent.click(await screen.findByRole("button", { name: "Rollback" }))
        await waitFor(() => expect(screen.getByRole("alert")).toHaveTextContent("second step failed"))
        expect(screen.getByText("Rollback selected changes")).toBeInTheDocument()
        expect(gitStatus).toHaveBeenCalled()
        expect(useUiStore.getState().gitChangeSelection).toEqual([])
    })

    it("cancel resolves false without IPC", async () => {
        const decision = openRollback(makeStatus({ untracked: ["scratch.txt"] }))
        fireEvent.click(await screen.findByRole("button", { name: "Cancel" }))
        await expect(decision).resolves.toBe(false)
        expect(gitRollbackPaths).not.toHaveBeenCalled()
    })

    it("Escape resolves the dialog as cancelled before execution", async () => {
        const decision = openRollback(makeStatus({ untracked: ["scratch.txt"] }))
        await screen.findByText("Rollback selected changes")
        fireEvent.keyDown(document, { key: "Escape" })
        await expect(decision).resolves.toBe(false)
        expect(gitRollbackPaths).not.toHaveBeenCalled()
    })

    it("resets the deletion checkbox to OFF for every request", async () => {
        const status = makeStatus({ untracked: ["scratch.txt"] })
        const first = openRollback(status)
        fireEvent.click(await screen.findByRole("checkbox"))
        expect(screen.getByRole("checkbox")).toBeChecked()
        fireEvent.click(screen.getByRole("button", { name: "Cancel" }))
        await expect(first).resolves.toBe(false)

        const second = useGitRollbackDialogStore.getState().request({
            repositoryRoot: "/w",
            targets: gitChangeRows(status)
        })
        expect(await screen.findByRole("checkbox")).not.toBeChecked()
        useGitRollbackDialogStore.getState().respond(false)
        await expect(second).resolves.toBe(false)
    })

    it("does not close a tab that becomes dirty while rollback IPC is running", async () => {
        const status = makeStatus({ untracked: ["race.txt"] })
        useWorkspaceStore.setState({
            groups: [{
                tabs: [{
                    path: "/w/race.txt",
                    name: "race.txt",
                    dirty: false,
                    externallyModified: false
                }],
                activePath: "/w/race.txt"
            }]
        })
        let resolveRollback!: (result: { restored: string[]; preservedUntracked: string[]; deleted: string[] }) => void
        gitRollbackPaths.mockImplementation(() => new Promise((resolve) => {
            resolveRollback = resolve
        }))
        const decision = openRollback(status)
        fireEvent.click(await screen.findByRole("checkbox"))
        fireEvent.click(screen.getByRole("button", { name: "Rollback" }))
        await waitFor(() => expect(gitRollbackPaths).toHaveBeenCalled())
        act(() => useWorkspaceStore.getState().markDirty("/w/race.txt", true))
        resolveRollback({ restored: [], preservedUntracked: [], deleted: ["race.txt"] })

        await expect(decision).resolves.toBe(true)
        expect(useWorkspaceStore.getState().groups[0].tabs[0]).toMatchObject({
            path: "/w/race.txt",
            dirty: true
        })
    })
})

import { fireEvent, render, screen, waitFor, within } from "@testing-library/react"
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"

vi.mock("@/lib/ipc", async (importOriginal) => ({
    ...(await importOriginal<typeof import("@/lib/ipc")>()),
    gitMergeBranch: vi.fn(async () => ({ conflicts: false })),
    gitDeleteBranch: vi.fn(async () => undefined),
    gitRenameBranch: vi.fn(async () => undefined),
    gitResetBranch: vi.fn(async () => undefined),
    gitStashList: vi.fn(async () => []),
    gitStashPush: vi.fn(async () => undefined),
    gitStashApply: vi.fn(async () => ({ conflicts: false })),
    gitStashDrop: vi.fn(async () => undefined)
}))
vi.mock("@/features/logs/userAction", () => ({ logUserAction: vi.fn(async () => undefined) }))
const dialogs = vi.hoisted(() => ({
    confirm: vi.fn(async () => true),
    textInput: vi.fn(async (): Promise<string | null> => null)
}))
vi.mock("@/state/appDialogStore", () => ({ requestAppConfirmation: dialogs.confirm, showAppMessage: vi.fn() }))
vi.mock("@/state/textInputDialogStore", () => ({ requestTextInputDialog: dialogs.textInput }))

import { resolveContextMenuEntries } from "@/app/workbench/contextMenuDefs"
import * as ipc from "@/lib/ipc"
import type { GitOperationOutcome, GitStatus } from "@/lib/types"
import { useGitActionDialogStore } from "@/state/gitActionDialogStore"
import { useGitConflictStore } from "@/state/gitConflictStore"
import { initialGitState, useGitStore } from "@/state/gitStore"
import { GitResetDialog } from "./GitResetDialog"
import { GitStashDialog } from "./GitStashDialog"
import { deleteBranch, mergeIntoCurrent, renameBranch, switchBranch } from "./gitOperations"

function status(overrides: Partial<GitStatus> = {}): GitStatus {
    return {
        branch: "main", headOid: "a".repeat(40), detached: false, upstream: null, ahead: 0, behind: 0,
        staged: [], unstaged: [], untracked: [], conflicted: [], inProgress: null, ...overrides
    }
}

// Runs the operation like the real store and records failures as lastError.
const runOp = vi.fn(async (_name: string, fn: () => Promise<unknown>) => {
    try {
        await fn()
        return true
    } catch (error) {
        useGitStore.setState({ lastError: String(error) })
        return false
    }
})

beforeEach(() => {
    useGitStore.setState({
        environment: { status: "ready", root: "/w", version: "2.50.1" },
        status: status(),
        runOp
    })
})
afterEach(() => {
    useGitStore.setState(initialGitState)
    useGitConflictStore.setState({ conflictsOpen: false, mergePath: null })
    useGitActionDialogStore.setState({ stashOpen: false, resetTarget: null })
    vi.clearAllMocks()
})

describe("branch operations", () => {
    it("opens the Conflicts dialog when a merge stops on conflicts", async () => {
        vi.mocked(ipc.gitMergeBranch).mockResolvedValueOnce({ conflicts: true })
        await mergeIntoCurrent("feature/x")
        expect(ipc.gitMergeBranch).toHaveBeenCalledWith("/w", "feature/x")
        expect(useGitConflictStore.getState().conflictsOpen).toBe(true)
    })

    it("offers a force delete only after git reports the branch is not fully merged", async () => {
        vi.mocked(ipc.gitDeleteBranch).mockRejectedValueOnce(new Error("error: the branch 'feature/x' is not fully merged"))
        await deleteBranch("feature/x")
        expect(ipc.gitDeleteBranch).toHaveBeenNthCalledWith(1, "/w", "feature/x", false)
        expect(ipc.gitDeleteBranch).toHaveBeenNthCalledWith(2, "/w", "feature/x", true)
        expect(dialogs.confirm).toHaveBeenCalledTimes(2)
    })

    it("does not offer a force delete for other failures", async () => {
        vi.mocked(ipc.gitDeleteBranch).mockRejectedValueOnce(new Error("error: branch 'feature/x' not found"))
        await deleteBranch("feature/x")
        expect(ipc.gitDeleteBranch).toHaveBeenCalledTimes(1)
    })

    it("renames only when a different name is entered", async () => {
        dialogs.textInput.mockResolvedValueOnce("  feature/x  ")
        await renameBranch("feature/x")
        expect(ipc.gitRenameBranch).not.toHaveBeenCalled()
        dialogs.textInput.mockResolvedValueOnce("feature/y")
        await renameBranch("feature/x")
        expect(ipc.gitRenameBranch).toHaveBeenCalledWith("/w", "feature/x", "feature/y")
    })

    it("hides merge and rebase for the current branch and while a conflict is open", () => {
        const labels = (isCurrent: boolean) => resolveContextMenuEntries({
            kind: "gitBranch", repositoryRoot: "/w", name: isCurrent ? "main" : "feature/x", branchKind: "local", isCurrent
        }).flatMap((entry) => entry.type === "command" ? [entry.label] : [])
        expect(labels(false)).toEqual(["Merge 'feature/x' into 'main'", "Rebase 'main' onto 'feature/x'", "Rename…", "Delete"])
        expect(labels(true)).toEqual(["Rename…"])
    })
})

describe("smart checkout", () => {
    const BLOCKED = "git switch: error: Your local changes to the following files would be overwritten by checkout:\n\ta.txt\nPlease commit your changes or stash them before you switch branches.\nAborting"

    it("offers Smart Checkout when local changes block the switch, then opens conflicts left by the restore", async () => {
        const run = vi.fn(async (smart: boolean): Promise<GitOperationOutcome> => {
            if (!smart) throw new Error(BLOCKED)
            return { conflicts: true }
        })
        expect(await switchBranch("/w", "checkout", "feature/x", run)).toBe(true)
        expect(run.mock.calls).toEqual([[false], [true]])
        expect(dialogs.confirm).toHaveBeenCalledWith(expect.objectContaining({
            title: "Git Checkout Problem",
            confirmLabel: "Smart Checkout",
            cancelLabel: "Don't Checkout"
        }))
        expect(useGitConflictStore.getState().conflictsOpen).toBe(true)
    })

    it("leaves other failures and a declined prompt alone", async () => {
        const invalid = vi.fn(async (): Promise<GitOperationOutcome> => {
            throw new Error("fatal: invalid reference: x")
        })
        expect(await switchBranch("/w", "checkout", "x", invalid)).toBe(false)
        expect(dialogs.confirm).not.toHaveBeenCalled()

        dialogs.confirm.mockResolvedValueOnce(false)
        const blocked = vi.fn(async (): Promise<GitOperationOutcome> => {
            throw new Error(BLOCKED)
        })
        expect(await switchBranch("/w", "checkout", "x", blocked)).toBe(false)
        expect(blocked).toHaveBeenCalledTimes(1)
        expect(useGitStore.getState().lastError).toContain("would be overwritten")
    })
})

describe("GitStashDialog", () => {
    it("stashes with the chosen options and pops with conflict handoff", async () => {
        vi.mocked(ipc.gitStashList).mockResolvedValue([{ index: 0, oid: "a".repeat(40), message: "On main: wip", timestamp: 1 }])
        vi.mocked(ipc.gitStashApply).mockResolvedValueOnce({ conflicts: true })
        useGitActionDialogStore.setState({ stashOpen: true })
        render(<GitStashDialog />)
        const dialog = await screen.findByRole("dialog")
        expect(await within(dialog).findByText("On main: wip")).toBeInTheDocument()

        fireEvent.change(within(dialog).getByLabelText("Message"), { target: { value: "half done" } })
        fireEvent.click(within(dialog).getByRole("checkbox", { name: "Keep staged changes" }))
        fireEvent.click(within(dialog).getByRole("button", { name: "Stash changes" }))
        await waitFor(() => expect(ipc.gitStashPush).toHaveBeenCalledWith("/w", "half done", true, true))

        fireEvent.click(within(dialog).getByRole("button", { name: "Pop stash@{0}" }))
        await waitFor(() => expect(ipc.gitStashApply).toHaveBeenCalledWith("/w", 0, "a".repeat(40), true))
        await waitFor(() => expect(useGitConflictStore.getState().conflictsOpen).toBe(true))
        expect(useGitActionDialogStore.getState().stashOpen).toBe(false)
    })

    it("shows the error and reloads the list when a stash changed under the dialog", async () => {
        vi.mocked(ipc.gitStashList).mockResolvedValue([{ index: 0, oid: "a".repeat(40), message: "On main: wip", timestamp: 1 }])
        vi.mocked(ipc.gitStashApply).mockRejectedValueOnce("git stash: stash@{0} changed since the list was loaded; reload and try again")
        useGitActionDialogStore.setState({ stashOpen: true })
        render(<GitStashDialog />)
        const dialog = await screen.findByRole("dialog")
        await within(dialog).findByText("On main: wip")
        const loads = vi.mocked(ipc.gitStashList).mock.calls.length

        fireEvent.click(within(dialog).getByRole("button", { name: "Apply stash@{0}" }))
        await waitFor(() => expect(useGitStore.getState().lastError).toContain("changed since the list"))
        await waitFor(() => expect(vi.mocked(ipc.gitStashList).mock.calls.length).toBeGreaterThan(loads))
        expect(useGitActionDialogStore.getState().stashOpen).toBe(true)
    })
})

describe("GitResetDialog", () => {
    it("resets the current branch with the selected mode", async () => {
        useGitActionDialogStore.setState({ resetTarget: { hash: "b".repeat(40), subject: "feat: x" } })
        render(<GitResetDialog />)
        const dialog = await screen.findByRole("dialog")
        expect(within(dialog).getByText("Reset main to here")).toBeInTheDocument()
        fireEvent.click(within(dialog).getByRole("radio", { name: /Hard/ }))
        fireEvent.click(within(dialog).getByRole("button", { name: "Reset" }))
        await waitFor(() => expect(ipc.gitResetBranch).toHaveBeenCalledWith("/w", "b".repeat(40), "hard"))
        expect(useGitActionDialogStore.getState().resetTarget).toBeNull()
    })
})

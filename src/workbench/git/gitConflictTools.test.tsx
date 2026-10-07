import { act, fireEvent, render, screen, waitFor, within } from "@testing-library/react"
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"

vi.mock("@/lib/ipc", async (importOriginal) => ({
    ...(await importOriginal<typeof import("@/lib/ipc")>()),
    gitConflictResolve: vi.fn(async () => undefined),
    gitConflictSides: vi.fn(),
    gitStage: vi.fn(async () => undefined),
    saveFile: vi.fn(async () => 0)
}))
vi.mock("@/features/logs/userAction", () => ({ logUserAction: vi.fn(async () => undefined) }))
vi.mock("@/lib/unsavedGuard", () => ({ dirtyTabPaths: vi.fn(() => []) }))

import * as ipc from "@/lib/ipc"
import { dirtyTabPaths } from "@/lib/unsavedGuard"
import { useAppDialogStore } from "@/state/appDialogStore"
import type { GitStatus } from "@/lib/types"
import { useGitConflictStore } from "@/state/gitConflictStore"
import { initialGitState, useGitStore } from "@/state/gitStore"
import { canMergeConflict, conflictSideChanges } from "./conflictSides"
import { GitConflictsDialog } from "./GitConflictsDialog"
import { GitMergeTool } from "./GitMergeTool"

function status(conflicted: GitStatus["conflicted"]): GitStatus {
    return {
        branch: "main", headOid: "0".repeat(40), detached: false, upstream: null, ahead: 0, behind: 0,
        staged: [], unstaged: [], untracked: [], conflicted, inProgress: "cherry-pick"
    }
}

const runOp = vi.fn(async (_name: string, fn: () => Promise<unknown>) => {
    await fn()
    return true
})

beforeEach(() => {
    useGitStore.setState({
        environment: { status: "ready", root: "/w", version: "2.50.1" },
        runOp
    })
})
afterEach(() => {
    useGitStore.setState(initialGitState)
    useGitConflictStore.setState({ conflictsOpen: false, mergePath: null })
    vi.clearAllMocks()
})

describe("conflict side changes", () => {
    it("reads ours from X and theirs from Y", () => {
        expect(conflictSideChanges("UD")).toEqual({ ours: "modified", theirs: "deleted" })
        expect(conflictSideChanges("AA")).toEqual({ ours: "added", theirs: "added" })
        expect(canMergeConflict("UU")).toBe(true)
        expect(canMergeConflict("DU")).toBe(false)
    })
})

describe("GitConflictsDialog", () => {
    it("accepts a whole side for the selected files", async () => {
        useGitStore.setState({ status: status([
            { path: "a.txt", origPath: null, status: "UU" },
            { path: "b.txt", origPath: null, status: "UD" }
        ]) })
        useGitConflictStore.setState({ conflictsOpen: true })
        render(<GitConflictsDialog />)

        const dialog = await screen.findByRole("dialog")
        expect(within(dialog).getAllByText("Deleted")).toHaveLength(1)
        fireEvent.click(within(dialog).getByRole("checkbox", { name: "Select a.txt" }))
        fireEvent.click(within(dialog).getByRole("button", { name: "Accept Theirs" }))
        await waitFor(() => expect(ipc.gitConflictResolve).toHaveBeenCalledWith("/w", ["a.txt"], "theirs"))
        expect(runOp).toHaveBeenCalledWith("conflict-accept-theirs", expect.any(Function))
    })

    it("maps Yours to the replayed commit, Git's theirs, during a rebase", async () => {
        useGitStore.setState({ status: { ...status([{ path: "b.txt", origPath: null, status: "UD" }]), inProgress: "rebase" } })
        useGitConflictStore.setState({ conflictsOpen: true })
        render(<GitConflictsDialog />)

        const dialog = await screen.findByRole("dialog")
        const cells = within(within(dialog).getAllByRole("row")[1]).getAllByRole("cell")
        expect(cells[2]).toHaveTextContent("Deleted")
        expect(cells[3]).toHaveTextContent("Modified")
        fireEvent.click(within(dialog).getByRole("button", { name: "Accept Yours" }))
        await waitFor(() => expect(ipc.gitConflictResolve).toHaveBeenCalledWith("/w", ["b.txt"], "theirs"))
    })

    it("only offers the merge tool when both sides kept the file", async () => {
        useGitStore.setState({ status: status([{ path: "b.txt", origPath: null, status: "UD" }]) })
        useGitConflictStore.setState({ conflictsOpen: true })
        render(<GitConflictsDialog />)
        expect(await screen.findByRole("button", { name: "Merge…" })).toBeDisabled()
    })

    it("closes once no conflicts remain", async () => {
        useGitStore.setState({ status: status([{ path: "a.txt", origPath: null, status: "UU" }]) })
        useGitConflictStore.setState({ conflictsOpen: true })
        render(<GitConflictsDialog />)
        await screen.findByRole("dialog")
        act(() => useGitStore.setState({ status: status([]) }))
        await waitFor(() => expect(useGitConflictStore.getState().conflictsOpen).toBe(false))
    })
})

describe("GitMergeTool", () => {
    it("merges non-conflicting changes, resolves a conflict per region and saves the result", async () => {
        vi.mocked(ipc.gitConflictSides).mockResolvedValue({
            code: "UU",
            base: { kind: "full", content: "1\n2\n3\n4\n5\n" },
            ours: { kind: "full", content: "1\nO2\n3\nO4\n5\n" },
            theirs: { kind: "full", content: "1\n2\n3\nT4\n5\n" },
            worktree: { kind: "full", content: "<<<<<<<\n" }
        })
        useGitConflictStore.setState({ mergePath: "f.txt" })
        render(<GitMergeTool />)

        expect(await screen.findByText("2 changes, 1 conflicts left")).toBeInTheDocument()
        fireEvent.click(screen.getByRole("button", { name: "Apply all non-conflicting" }))
        expect(await screen.findByText("1 changes, 1 conflicts left")).toBeInTheDocument()

        const result = document.querySelector<HTMLElement>("[data-merge-pane='result']")!
        fireEvent.click(within(result).getByRole("button", { name: "Theirs »" }))
        expect(await screen.findByText("0 changes, 0 conflicts left")).toBeInTheDocument()

        fireEvent.click(screen.getByRole("button", { name: "Apply" }))
        await waitFor(() => expect(ipc.saveFile).toHaveBeenCalledWith("/w/f.txt", "1\nO2\n3\nT4\n5\n"))
        expect(ipc.gitStage).toHaveBeenCalledWith("/w", ["f.txt"])
        expect(useGitConflictStore.getState().mergePath).toBeNull()
    })

    it("keeps CRLF line endings of the working file", async () => {
        vi.mocked(ipc.gitConflictSides).mockResolvedValue({
            code: "UU",
            base: { kind: "full", content: "a\nb\n" },
            ours: { kind: "full", content: "a\nB\n" },
            theirs: { kind: "full", content: "a\nb\n" },
            worktree: { kind: "full", content: "a\r\n<<<<<<<\r\n" }
        })
        useGitConflictStore.setState({ mergePath: "w.txt" })
        render(<GitMergeTool />)
        await screen.findByText("1 changes, 0 conflicts left")
        fireEvent.click(screen.getByRole("button", { name: "Apply non-conflicting from left" }))
        await screen.findByText("0 changes, 0 conflicts left")
        fireEvent.click(screen.getByRole("button", { name: "Apply" }))
        await waitFor(() => expect(ipc.saveFile).toHaveBeenCalledWith("/w/w.txt", "a\r\nB\r\n"))
    })

    async function resolveAndApply(worktree: string) {
        vi.mocked(ipc.gitConflictSides).mockResolvedValue({
            code: "UU",
            base: { kind: "full", content: "1\n2\n" },
            ours: { kind: "full", content: "1\nO\n" },
            theirs: { kind: "full", content: "1\nT\n" },
            worktree: { kind: "full", content: worktree }
        })
        useGitConflictStore.setState({ mergePath: "m.txt" })
        render(<GitMergeTool />)
        await screen.findByText("1 changes, 1 conflicts left")
        const result = document.querySelector<HTMLElement>("[data-merge-pane='result']")!
        fireEvent.click(within(result).getByRole("button", { name: "Theirs »" }))
        await screen.findByText("0 changes, 0 conflicts left")
        fireEvent.click(screen.getByRole("button", { name: "Apply" }))
    }

    it("asks before replacing conflicts already resolved by hand", async () => {
        await resolveAndApply("1\nmine\n")
        await waitFor(() => expect(useAppDialogStore.getState().pending?.description).toMatch(/manual resolutions/))
        act(() => useAppDialogStore.getState().respond(false))
        await waitFor(() => expect(useAppDialogStore.getState().pending).toBeNull())
        expect(ipc.saveFile).not.toHaveBeenCalled()

        fireEvent.click(screen.getByRole("button", { name: "Apply" }))
        await waitFor(() => expect(useAppDialogStore.getState().pending).not.toBeNull())
        act(() => useAppDialogStore.getState().respond(true))
        await waitFor(() => expect(ipc.saveFile).toHaveBeenCalledWith("/w/m.txt", "1\nT\n"))
    })

    it("asks before replacing unsaved editor changes to the file", async () => {
        vi.mocked(dirtyTabPaths).mockReturnValue(["/w/m.txt"])
        await resolveAndApply("1\n<<<<<<< ours\nO\n=======\nT\n>>>>>>> theirs\n")
        await waitFor(() => expect(useAppDialogStore.getState().pending?.description).toMatch(/unsaved changes/))
        act(() => useAppDialogStore.getState().respond(false))
        await waitFor(() => expect(useAppDialogStore.getState().pending).toBeNull())
        expect(ipc.saveFile).not.toHaveBeenCalled()
        vi.mocked(dirtyTabPaths).mockReturnValue([])
    })

    it("applies without asking while the file still holds Git's conflict markers", async () => {
        await resolveAndApply("1\n<<<<<<< ours\nO\n=======\nT\n>>>>>>> theirs\n")
        await waitFor(() => expect(ipc.saveFile).toHaveBeenCalledWith("/w/m.txt", "1\nT\n"))
        expect(useAppDialogStore.getState().pending).toBeNull()
    })

    it("shows the replayed commit as Yours while rebasing", async () => {
        useGitStore.setState({ status: { ...status([{ path: "r.txt", origPath: null, status: "UU" }]), inProgress: "rebase" } })
        vi.mocked(ipc.gitConflictSides).mockResolvedValue({
            code: "UU",
            base: { kind: "full", content: "1\n2\n" },
            ours: { kind: "full", content: "1\nupstream\n" },
            theirs: { kind: "full", content: "1\nmine\n" },
            worktree: { kind: "full", content: "1\n<<<<<<< upstream\n" }
        })
        useGitConflictStore.setState({ mergePath: "r.txt" })
        render(<GitMergeTool />)
        await screen.findByText("1 changes, 1 conflicts left")
        expect(document.querySelector("[data-merge-pane='yours']")).toHaveTextContent("mine")
        expect(document.querySelector("[data-merge-pane='theirs']")).toHaveTextContent("upstream")
    })

    it("explains why a deleted side cannot be merged", async () => {
        vi.mocked(ipc.gitConflictSides).mockResolvedValue({
            code: "UD", base: { kind: "full", content: "x\n" }, ours: { kind: "full", content: "y\n" }, theirs: null, worktree: null
        })
        useGitConflictStore.setState({ mergePath: "d.txt" })
        render(<GitMergeTool />)
        expect(await screen.findByRole("alert")).toHaveTextContent("One side deleted this file")
    })
})

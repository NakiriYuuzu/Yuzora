import { act, cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react"
import { EditorView } from "@codemirror/view"
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"

vi.mock("@/lib/ipc", async (importOriginal) => ({
    ...(await importOriginal<typeof import("@/lib/ipc")>()),
    gitConflictSides: vi.fn(),
    gitStage: vi.fn(async () => undefined),
    saveFile: vi.fn(async () => 0)
}))
vi.mock("@/features/logs/userAction", () => ({ logUserAction: vi.fn(async () => undefined) }))
vi.mock("@/lib/unsavedGuard", () => ({ dirtyTabPaths: vi.fn(() => []) }))

import * as ipc from "@/lib/ipc"
import i18n from "@/lib/i18n"
import type { GitConflictSides, GitStatus } from "@/lib/types"
import { dirtyTabPaths } from "@/lib/unsavedGuard"
import { useAppDialogStore } from "@/state/appDialogStore"
import { useGitConflictStore } from "@/state/gitConflictStore"
import { initialGitState, useGitStore } from "@/state/gitStore"
import { GitMergeTool } from "./GitMergeTool"

const originalRunOp = useGitStore.getState().runOp
const originalRefresh = useGitStore.getState().refresh
const originalLoadBranches = useGitStore.getState().loadBranches
const sides: GitConflictSides = {
    code: "UU", base: { kind: "full", content: "base\n" },
    ours: { kind: "full", content: "ours\n" }, theirs: { kind: "full", content: "theirs\n" },
    worktree: { kind: "full", content: "<<<<<<< HEAD\nours\n=======\ntheirs\n>>>>>>> theirs\n" }
}
const status: GitStatus = {
    branch: "fixture", headOid: "0".repeat(40), detached: false, upstream: null, ahead: 0, behind: 0,
    staged: [], unstaged: [], untracked: [], conflicted: [{ path: "f.txt", status: "UU", origPath: null }], inProgress: "merge"
}

function deferred<T>() {
    let resolve!: (value: T) => void
    const promise = new Promise<T>((done) => { resolve = done })
    return { promise, resolve }
}

beforeEach(() => {
    vi.mocked(ipc.gitConflictSides).mockReset().mockResolvedValue(sides)
    vi.mocked(ipc.gitStage).mockReset().mockResolvedValue(undefined)
    vi.mocked(ipc.saveFile).mockReset().mockResolvedValue(0)
    useGitStore.setState({ ...initialGitState, environment: { status: "ready", root: "/owned-merge", version: "2.50.1" },
        status, runOp: originalRunOp, refresh: vi.fn(async () => undefined), loadBranches: vi.fn(async () => undefined) })
})
afterEach(async () => {
    cleanup()
    act(() => useAppDialogStore.getState().respond(false))
    useGitConflictStore.setState({ conflictsOpen: false, mergePath: null })
    useGitStore.setState({ ...initialGitState, refresh: originalRefresh, loadBranches: originalLoadBranches })
    vi.mocked(dirtyTabPaths).mockReturnValue([])
    vi.clearAllMocks()
    await i18n.changeLanguage("en")
})

async function openResolved() {
    useGitConflictStore.setState({ mergePath: "f.txt" })
    render(<GitMergeTool />)
    await screen.findByText("1 changes, 1 conflicts left")
    const pane = document.querySelector<HTMLElement>("[data-merge-pane='result']")!
    fireEvent.click(within(pane).getByRole("button", { name: "Theirs »" }))
    await screen.findByText("0 changes, 0 conflicts left")
    return pane
}
function apply() { fireEvent.click(screen.getByRole("button", { name: "Apply" })) }
async function expectBlocked(error: RegExp) {
    await waitFor(() => expect(useGitStore.getState().lastError).toMatch(error))
    expect(ipc.saveFile).not.toHaveBeenCalled()
    expect(ipc.gitStage).not.toHaveBeenCalled()
    expect(useGitConflictStore.getState().mergePath).toBe("f.txt")
    expect(useGitStore.getState().consoleLog[0]?.tone).toBe("err")
}

// The "not in conflict" error follows the backend's status lookup after an
// actual owned Git merge --abort removed all unmerged stages (experiment 078).
describe("merge apply preflight", () => {
    for (const error of ["git: f.txt is not in conflict", "temporary helper read failure"]) {
        it(`does not overwrite or stage when preflight rejects: ${error}`, async () => {
            await openResolved()
            vi.mocked(ipc.gitConflictSides).mockRejectedValueOnce(new Error(error))
            apply()
            await expectBlocked(new RegExp(error))
            apply()
            await waitFor(() => expect(ipc.gitStage).toHaveBeenCalledWith("/owned-merge", ["f.txt"]))
            expect(ipc.saveFile).toHaveBeenCalledWith("/owned-merge/f.txt", "theirs\n")
        })
    }
    for (const field of ["base", "ours", "theirs", "code"] as const) {
        it(`rejects another conflict at the same path when ${field} changes`, async () => {
            await openResolved()
            const changed = { ...sides, [field]: field === "code" ? "AA" : { kind: "full", content: `new ${field}\n` } }
            vi.mocked(ipc.gitConflictSides).mockResolvedValue(changed as GitConflictSides)
            apply()
            await expectBlocked(/changed|out of date/i)
        })
    }
    it("compares index inputs before logical LF normalization", async () => {
        await openResolved()
        vi.mocked(ipc.gitConflictSides).mockResolvedValue({ ...sides, base: { kind: "full", content: "base\r\n" } })
        apply()
        await expectBlocked(/changed|out of date/i)
    })
    it("does not reuse consent when the worktree changes again during confirmation", async () => {
        await openResolved()
        vi.mocked(ipc.gitConflictSides).mockResolvedValueOnce({ ...sides, worktree: { kind: "full", content: "manual one\n" } })
            .mockResolvedValue({ ...sides, worktree: { kind: "full", content: "manual two\n" } })
        apply()
        await waitFor(() => expect(useAppDialogStore.getState().pending?.description).toMatch(/manual resolutions/))
        act(() => useAppDialogStore.getState().respond(true))
        await expectBlocked(/changed|out of date/i)
    })
    it("accepts an unchanged acknowledged manual worktree", async () => {
        await openResolved()
        vi.mocked(ipc.gitConflictSides).mockResolvedValue({ ...sides, worktree: { kind: "full", content: "manual\n" } })
        apply()
        await waitFor(() => expect(useAppDialogStore.getState().pending).not.toBeNull())
        act(() => useAppDialogStore.getState().respond(true))
        await waitFor(() => expect(ipc.gitStage).toHaveBeenCalledOnce())
        expect(ipc.saveFile).toHaveBeenCalledWith("/owned-merge/f.txt", "theirs\n")
    })
    it("does not record success or close when confirmation is cancelled", async () => {
        await openResolved()
        vi.mocked(dirtyTabPaths).mockReturnValue(["/owned-merge/f.txt"])
        apply()
        await waitFor(() => expect(useAppDialogStore.getState().pending).not.toBeNull())
        await act(async () => useAppDialogStore.getState().respond(false))
        expect(ipc.saveFile).not.toHaveBeenCalled()
        expect(ipc.gitStage).not.toHaveBeenCalled()
        expect(useGitStore.getState().consoleLog).toHaveLength(0)
        expect(useGitConflictStore.getState().mergePath).toBe("f.txt")
    })
    it("allows a stash conflict with no in-progress merge operation", async () => {
        useGitStore.setState({ status: { ...status, inProgress: null } })
        await openResolved()
        apply()
        await waitFor(() => expect(ipc.gitStage).toHaveBeenCalledOnce())
        expect(ipc.gitConflictSides).toHaveBeenCalledTimes(2)
    })
    it("prevents concurrent Apply requests while the preflight is pending", async () => {
        await openResolved()
        const pending = deferred<GitConflictSides>()
        vi.mocked(ipc.gitConflictSides).mockImplementation(() => pending.promise)
        apply()
        apply()
        await act(async () => pending.resolve(sides))
        await waitFor(() => expect(ipc.gitStage).toHaveBeenCalledOnce())
        expect(ipc.saveFile).toHaveBeenCalledOnce()
        expect(ipc.gitConflictSides).toHaveBeenCalledTimes(2)
    })
    it("rejects a result edited while the preflight is pending", async () => {
        const pane = await openResolved()
        const pending = deferred<GitConflictSides>()
        vi.mocked(ipc.gitConflictSides).mockReturnValue(pending.promise)
        apply()
        const view = EditorView.findFromDOM(pane.querySelector(".cm-editor")!)!
        act(() => view.dispatch({ changes: { from: 0, to: view.state.doc.length, insert: "newer result\n" } }))
        await act(async () => pending.resolve(sides))
        await expectBlocked(/changed|out of date/i)
    })
    it("does not write an old view after switching repository with the same path", async () => {
        await openResolved()
        const pending = deferred<GitConflictSides>()
        vi.mocked(ipc.gitConflictSides).mockReturnValueOnce(pending.promise).mockResolvedValue(sides)
        apply()
        act(() => useGitStore.setState({ environment: { status: "ready", root: "/other-owned-merge", version: "2.50.1" } }))
        await screen.findByText("1 changes, 1 conflicts left")
        await act(async () => pending.resolve(sides))
        expect(ipc.saveFile).not.toHaveBeenCalled()
        expect(ipc.gitStage).not.toHaveBeenCalled()
        expect(useGitConflictStore.getState().mergePath).toBe("f.txt")
    })
    it("does not stage or close a reopened dialog when an old save completes", async () => {
        await openResolved()
        const saving = deferred<number>()
        vi.mocked(ipc.saveFile).mockReturnValueOnce(saving.promise)
        apply()
        await waitFor(() => expect(ipc.saveFile).toHaveBeenCalledOnce())
        act(() => useGitConflictStore.getState().closeMerge())
        act(() => useGitConflictStore.getState().openMerge("f.txt"))
        await screen.findByText("1 changes, 1 conflicts left")
        await act(async () => saving.resolve(0))
        expect(ipc.gitStage).not.toHaveBeenCalled()
        expect(useGitConflictStore.getState().mergePath).toBe("f.txt")
    })
    for (const base of [null, { kind: "limited", content: "base\n" }] as const) {
        it(`rejects changed stage presence or kind: ${base?.kind ?? "absent"}`, async () => {
            await openResolved()
            vi.mocked(ipc.gitConflictSides).mockResolvedValue({ ...sides, base })
            apply()
            await expectBlocked(/changed|out of date/i)
        })
    }
    it("does not apply after the conflict disappears while confirmation is open", async () => {
        await openResolved()
        vi.mocked(dirtyTabPaths).mockReturnValue(["/owned-merge/f.txt"])
        apply()
        await waitFor(() => expect(useAppDialogStore.getState().pending).not.toBeNull())
        vi.mocked(ipc.gitConflictSides).mockRejectedValueOnce(new Error("git: f.txt is not in conflict"))
        act(() => useAppDialogStore.getState().respond(true))
        await expectBlocked(/not in conflict/)
    })
    it("retires the old side orientation when rebase identity changes during preflight", async () => {
        await openResolved()
        const pending = deferred<GitConflictSides>()
        vi.mocked(ipc.gitConflictSides).mockReturnValueOnce(pending.promise).mockResolvedValue(sides)
        apply()
        act(() => useGitStore.setState({ status: { ...status, inProgress: "rebase" } }))
        await screen.findByText("1 changes, 1 conflicts left")
        expect(document.querySelector("[data-merge-pane='yours']")).toHaveTextContent("theirs")
        await act(async () => pending.resolve(sides))
        expect(ipc.saveFile).not.toHaveBeenCalled()
        expect(ipc.gitStage).not.toHaveBeenCalled()
    })
    it("does not close a reopened dialog after an old stage finishes", async () => {
        await openResolved()
        const staging = deferred<void>()
        vi.mocked(ipc.gitStage).mockReturnValueOnce(staging.promise)
        apply()
        await waitFor(() => expect(ipc.gitStage).toHaveBeenCalledOnce())
        act(() => useGitConflictStore.getState().closeMerge())
        act(() => useGitConflictStore.getState().openMerge("f.txt"))
        await screen.findByText("1 changes, 1 conflicts left")
        await act(async () => staging.resolve())
        expect(useGitConflictStore.getState().mergePath).toBe("f.txt")
    })
    it("keeps newer result edits when the previous stage finishes", async () => {
        const pane = await openResolved()
        const staging = deferred<void>()
        vi.mocked(ipc.gitStage).mockReturnValueOnce(staging.promise)
        apply()
        await waitFor(() => expect(ipc.gitStage).toHaveBeenCalledOnce())
        const view = EditorView.findFromDOM(pane.querySelector(".cm-editor")!)!
        act(() => view.dispatch({ changes: { from: 0, to: view.state.doc.length, insert: "newer result\n" } }))
        await act(async () => staging.resolve())
        expect(useGitConflictStore.getState().mergePath).toBe("f.txt")
        expect(view.state.doc.toString()).toBe("newer result\n")
    })
    it("checks the latest conflict after unresolved confirmation", async () => {
        useGitConflictStore.setState({ mergePath: "f.txt" })
        render(<GitMergeTool />)
        await screen.findByText("1 changes, 1 conflicts left")
        apply()
        await waitFor(() => expect(useAppDialogStore.getState().pending?.description).toMatch(/unprocessed/))
        vi.mocked(ipc.gitConflictSides).mockRejectedValueOnce(new Error("git: f.txt is not in conflict"))
        act(() => useAppDialogStore.getState().respond(true))
        await expectBlocked(/not in conflict/)
    })
    it("releases saving state when language changes rebuild the pending view", async () => {
        await openResolved()
        const pending = deferred<GitConflictSides>()
        vi.mocked(ipc.gitConflictSides).mockReturnValueOnce(pending.promise).mockResolvedValue(sides)
        apply()
        await act(async () => { await i18n.changeLanguage("zh-TW") })
        await screen.findByText("剩 1 個變更，其中 1 個衝突")
        await act(async () => pending.resolve(sides))
        expect(ipc.saveFile).not.toHaveBeenCalled()
        expect(ipc.gitStage).not.toHaveBeenCalled()
        const applyButton = screen.getByRole("button", { name: "套用" })
        expect(applyButton).toBeEnabled()
        const pane = document.querySelector<HTMLElement>("[data-merge-pane='result']")!
        fireEvent.click(within(pane).getByRole("button", { name: "Theirs »" }))
        fireEvent.click(applyButton)
        await waitFor(() => expect(ipc.gitStage).toHaveBeenCalledOnce())
        expect(ipc.saveFile).toHaveBeenCalledWith("/owned-merge/f.txt", "theirs\n")
    })
    for (const operation of ["saveFile", "gitStage"] as const) {
        it(`keeps the result open when ${operation} fails`, async () => {
            await openResolved()
            vi.mocked(ipc[operation]).mockRejectedValueOnce(new Error(`${operation} failed`))
            apply()
            await waitFor(() => expect(useGitStore.getState().lastError).toContain(`${operation} failed`))
            expect(useGitConflictStore.getState().mergePath).toBe("f.txt")
            if (operation === "saveFile") expect(ipc.gitStage).not.toHaveBeenCalled()
        })
    }
})

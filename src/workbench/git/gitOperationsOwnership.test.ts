import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"

vi.mock("@/lib/ipc", async importOriginal => ({
    ...await importOriginal<typeof import("@/lib/ipc")>(),
    gitBootstrap: vi.fn(),
    gitStatus: vi.fn(),
    gitBranches: vi.fn(),
    gitRenameBranch: vi.fn(),
    gitDeleteBranch: vi.fn(),
    gitLogPage: vi.fn(),
    gitLogAuthors: vi.fn(),
    gitCommitDetail: vi.fn()
}))
vi.mock("@/features/logs/userAction", () => ({ logUserAction: vi.fn(async () => undefined) }))

import { logUserAction } from "@/features/logs/userAction"
import * as ipc from "@/lib/ipc"
import type { BranchList, CommitDetail, GitStatus, LogPage } from "@/lib/types"
import * as confirmations from "@/state/appDialogStore"
import { useGitLogStore } from "@/state/gitLogStore"
import { clearGitSnapshots, initialGitState, useGitStore } from "@/state/gitStore"
import * as textInputs from "@/state/textInputDialogStore"
import { deleteBranch, renameBranch } from "./gitOperations"

const branches = (): BranchList => ({ local: [], remote: [], tags: [] })
const status = (): GitStatus => ({
    branch: "main", headOid: "a".repeat(40), detached: false, upstream: null,
    ahead: 0, behind: 0, staged: [], unstaged: [], untracked: [], conflicted: [], inProgress: null
})
const page = (root: string): LogPage => ({
    commits: [{ hash: root, shortHash: root, subject: root, authorName: "Fixture", authorEmail: "fixture@example.com", timestamp: 1, parents: [], refs: [] }],
    hasMore: false, nextCursor: null
})
const detail = (root: string): CommitDetail => ({
    subject: root, body: root, authorName: "Fixture", authorEmail: "fixture@example.com",
    timestamp: 1, parents: [], files: [], totalAdditions: 0, totalDeletions: 0
})
function deferred<T>() {
    let resolve!: (value: T) => void
    let reject!: (error: Error) => void
    const promise = new Promise<T>((yes, no) => { resolve = yes; reject = no })
    return { promise, resolve, reject }
}

// Reproduce the repository transition and LogTab's reset/load/selection without
// replacing runOp, refresh, loadBranches, or the history store with stubs.
async function selectRepository(root: string) {
    await useGitStore.getState().detect(root)
    useGitLogStore.getState().reset({ preserveAuthorSession: true })
    useGitLogStore.setState({ filters: { query: root, author: null, since: null, until: null } })
    await useGitLogStore.getState().loadFirstPage(root)
    await useGitLogStore.getState().select(root, root)
}
const operations = ["rename", "delete", "force-delete"] as const
type Operation = typeof operations[number]
function startOperation(kind: Operation, pending?: Promise<void>) {
    if (kind === "rename") {
        if (pending) vi.mocked(ipc.gitRenameBranch).mockReturnValueOnce(pending)
        return renameBranch("feature/old")
    }
    if (kind === "force-delete") {
        vi.mocked(ipc.gitDeleteBranch).mockRejectedValueOnce(new Error("branch is not fully merged"))
    }
    if (pending) vi.mocked(ipc.gitDeleteBranch).mockReturnValueOnce(pending)
    return deleteBranch("feature/old")
}
function expectMutationCalls(kind: Operation) {
    if (kind === "rename") {
        expect(ipc.gitRenameBranch).toHaveBeenCalledExactlyOnceWith("/a", "feature/old", "feature/new")
        expect(ipc.gitDeleteBranch).not.toHaveBeenCalled()
    } else {
        expect(ipc.gitDeleteBranch).toHaveBeenNthCalledWith(1, "/a", "feature/old", false)
        expect(ipc.gitDeleteBranch).toHaveBeenCalledTimes(kind === "force-delete" ? 2 : 1)
        if (kind === "force-delete") expect(ipc.gitDeleteBranch).toHaveBeenNthCalledWith(2, "/a", "feature/old", true)
    }
}

beforeEach(async () => {
    vi.useFakeTimers()
    vi.resetAllMocks()
    clearGitSnapshots()
    useGitStore.setState(initialGitState)
    useGitLogStore.getState().reset()
    vi.spyOn(confirmations, "requestAppConfirmation").mockResolvedValue(true)
    vi.spyOn(textInputs, "requestTextInputDialog").mockResolvedValue("feature/new")
    vi.mocked(ipc.gitBootstrap).mockImplementation(async root => ({ environment: { status: "ready", root, version: "2.50.1" }, status: status(), branches: branches() }))
    vi.mocked(ipc.gitStatus).mockImplementation(async () => status())
    vi.mocked(ipc.gitBranches).mockImplementation(async () => branches())
    vi.mocked(ipc.gitRenameBranch).mockResolvedValue(undefined)
    vi.mocked(ipc.gitDeleteBranch).mockResolvedValue(undefined)
    vi.mocked(ipc.gitLogPage).mockImplementation(async root => page(root))
    vi.mocked(ipc.gitLogAuthors).mockResolvedValue([])
    vi.mocked(ipc.gitCommitDetail).mockImplementation(async root => detail(root))
    await selectRepository("/a")
    vi.mocked(ipc.gitLogPage).mockClear()
})
afterEach(async () => {
    await vi.runOnlyPendingTimersAsync()
    clearGitSnapshots()
    useGitStore.setState(initialGitState)
    useGitLogStore.getState().reset()
    vi.restoreAllMocks()
    vi.useRealTimers()
})

describe.each(operations)("%s history ownership", kind => {
    for (const phase of ["mutation", "post-refresh"] as const) {
        for (const transition of ["A→B", "A→B→A"] as const) {
            it(`preserves the new selection after late ${phase} on ${transition}`, async () => {
                const mutation = deferred<void>(), branchRead = deferred<BranchList>()
                if (phase === "post-refresh") vi.mocked(ipc.gitBranches).mockReturnValueOnce(branchRead.promise)
                const operation = startOperation(kind, phase === "mutation" ? mutation.promise : undefined)
                await vi.advanceTimersByTimeAsync(350)
                expectMutationCalls(kind)
                if (phase === "mutation") expect(ipc.gitBranches).not.toHaveBeenCalled()
                else expect(ipc.gitBranches).toHaveBeenCalledExactlyOnceWith("/a")
                await selectRepository("/b")
                if (transition === "A→B→A") await selectRepository("/a")
                const selected = useGitLogStore.getState()
                const requestCount = vi.mocked(ipc.gitLogPage).mock.calls.length
                mutation.resolve(undefined)
                branchRead.resolve(branches())
                await operation
                await vi.advanceTimersByTimeAsync(350)
                expect.soft(ipc.gitLogPage).toHaveBeenCalledTimes(requestCount)
                expect(useGitLogStore.getState()).toMatchObject({
                    commits: selected.commits, selectedHash: selected.selectedHash,
                    detail: selected.detail, filters: selected.filters, listError: null, detailError: null
                })
                expect(useGitStore.getState().busy).toBeNull()
            })
        }
    }

    it("refreshes refs once after success without changing mutation or logging semantics", async () => {
        const mutation = deferred<void>()
        const operation = startOperation(kind, mutation.promise)
        await vi.advanceTimersByTimeAsync(0)
        expectMutationCalls(kind)
        expect(ipc.gitLogPage).not.toHaveBeenCalled()
        mutation.resolve(undefined)
        await vi.advanceTimersByTimeAsync(350)
        await operation
        expect(ipc.gitLogPage).toHaveBeenCalledTimes(1)
        expect(vi.mocked(ipc.gitLogPage).mock.calls[0][0]).toBe("/a")
        expect(useGitStore.getState().busy).toBeNull()
        expect(logUserAction).toHaveBeenCalledWith(kind === "rename" ? "git_rename_branch" : "git_delete_branch", expect.any(String))
    })

    it("does not reload refs on mutation failure", async () => {
        const mutation = deferred<void>()
        const operation = startOperation(kind, mutation.promise)
        await vi.advanceTimersByTimeAsync(0)
        mutation.reject(new Error("fixture mutation failed"))
        await operation
        expectMutationCalls(kind)
        expect(ipc.gitLogPage).not.toHaveBeenCalled()
        expect(useGitStore.getState().lastError).toContain("fixture mutation failed")
    })

    it("keeps history failure separate from successful mutation", async () => {
        vi.mocked(ipc.gitLogPage).mockRejectedValueOnce(new Error("fixture history unavailable"))
        const operation = startOperation(kind)
        await vi.advanceTimersByTimeAsync(350)
        await operation
        expectMutationCalls(kind)
        expect(useGitLogStore.getState().listError).toContain("fixture history unavailable")
        expect(useGitStore.getState()).toMatchObject({ busy: null, lastError: null })
        expect(logUserAction).toHaveBeenCalledWith(kind === "rename" ? "git_rename_branch" : "git_delete_branch", expect.any(String))
    })
})

it.each(["rename", "delete", "force-delete"] as const)("does not mutate or reload after %s confirmation is declined", async kind => {
    if (kind === "rename") vi.mocked(textInputs.requestTextInputDialog).mockResolvedValueOnce(null)
    else if (kind === "delete") vi.mocked(confirmations.requestAppConfirmation).mockResolvedValueOnce(false)
    else vi.mocked(confirmations.requestAppConfirmation).mockResolvedValueOnce(true).mockResolvedValueOnce(false)
    await startOperation(kind)
    expect(ipc.gitLogPage).not.toHaveBeenCalled()
    expect(ipc.gitRenameBranch).not.toHaveBeenCalled()
    expect(ipc.gitDeleteBranch).toHaveBeenCalledTimes(kind === "force-delete" ? 1 : 0)
})

it("releases the mutation gate while history is still pending", async () => {
    const history = deferred<LogPage>()
    vi.mocked(ipc.gitLogPage).mockReturnValueOnce(history.promise)
    let done = false
    const operation = renameBranch("feature/old").then(() => { done = true })
    try {
        await vi.advanceTimersByTimeAsync(350)
        expect(done).toBe(true)
        expect(useGitStore.getState().busy).toBeNull()
        expect(useGitLogStore.getState().loading).toBe(true)
    } finally {
        history.resolve(page("/a"))
        await vi.advanceTimersByTimeAsync(350)
        await operation
    }
})

it.each(["resolve", "reject"] as const)("ignores a late %s from history started before a repository switch", async outcome => {
    const history = deferred<LogPage>()
    vi.mocked(ipc.gitLogPage).mockReturnValueOnce(history.promise)
    const operation = renameBranch("feature/old")
    await vi.advanceTimersByTimeAsync(350)
    await operation
    await selectRepository("/b")
    const selected = useGitLogStore.getState()
    if (outcome === "resolve") history.resolve(page("/a"))
    else history.reject(new Error("old A history failed"))
    await vi.advanceTimersByTimeAsync(0)
    expect(useGitLogStore.getState()).toMatchObject({
        commits: selected.commits, selectedHash: selected.selectedHash,
        detail: selected.detail, filters: selected.filters, loading: false, listError: null
    })
})

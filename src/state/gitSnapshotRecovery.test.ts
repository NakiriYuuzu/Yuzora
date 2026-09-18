import { beforeEach, expect, it, vi } from "vitest"
vi.mock("@/lib/ipc", () => ({ gitBootstrap: vi.fn(), gitBranches: vi.fn(), gitStatus: vi.fn(), gitFetch: vi.fn(), gitRemoteProbe: vi.fn(), gitCommitDetail: vi.fn() }))
import { gitBranches, gitStatus } from "@/lib/ipc"
import { clearGitSnapshots, initialGitState, useGitStore } from "./gitStore"

beforeEach(() => {
    clearGitSnapshots()
    useGitStore.setState({ ...initialGitState, environment: { status: "ready", root: "/w", version: "2.50" } })
    vi.mocked(gitStatus).mockResolvedValue({ branch: "main", headOid: "a".repeat(40), detached: false, upstream: null, ahead: 0, behind: 0, staged: [], unstaged: [], untracked: [], conflicted: [], inProgress: null })
    vi.mocked(gitBranches).mockResolvedValue({ local: [], remote: [], tags: [] })
})

it("does not report a successful mutation as failed when its snapshot meets transient job contention", async () => {
    vi.mocked(gitBranches).mockRejectedValueOnce("git-jobs-busy")
    const mutate = vi.fn(async () => {})
    expect(await useGitStore.getState().runOp("fetch", mutate)).toBe(true)
    expect(mutate).toHaveBeenCalledOnce()
    expect(useGitStore.getState().lastError).toBeNull()
    expect(useGitStore.getState().snapshotStale).toBe(false)
})

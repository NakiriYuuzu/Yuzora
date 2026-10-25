import { beforeEach, describe, expect, it, vi } from "vitest"
import { dropLog, loadCommit, loadDiff, loadLog, loadStatus } from "./gitReads"
import { spaceKey } from "./lib/git"
import type { GitCommit, GitLog, GitLogCommit, Snapshot } from "./lib/protocol"
import { RpcError } from "./lib/rpc"
import { useCompanion } from "./store"

const client = vi.hoisted(() => ({ call: vi.fn() }))
vi.mock("./client", () => ({ call: client.call }))

const commit = (hash: string, parents: string[] = []): GitLogCommit => ({
  hash, shortHash: hash.slice(0, 7), subject: `commit ${hash}`, authorName: "Sora", authorEmail: "sora@example.com", timestamp: 1_760_000_000, parents, refs: [],
})
const page = (hashes: string[], nextCursor: string | null): GitLog => ({ commits: hashes.map((hash) => commit(hash)), hasMore: nextCursor !== null, nextCursor })
const log = () => useCompanion.getState().git[spaceKey("work", "w1")]?.log

beforeEach(() => {
  client.call.mockReset()
  useCompanion.setState({ git: {}, notice: null })
})

describe("loadLog", () => {
  it("appends the next page with the cursor of the last one", async () => {
    client.call.mockResolvedValueOnce(page(["a", "b"], "c1")).mockResolvedValueOnce(page(["c"], null))
    await loadLog("work", "w1")
    await loadLog("work", "w1", true)
    expect(client.call.mock.calls).toEqual([
      ["git.log", { session: "work", workspaceId: "w1" }],
      ["git.log", { session: "work", workspaceId: "w1", cursor: "c1" }],
    ])
    expect(log()).toMatchObject({ hasMore: false, nextCursor: null })
    expect(log()?.commits.map((item) => item.hash)).toEqual(["a", "b", "c"])
  })

  it("starts over from the first page when the cursor is no longer valid", async () => {
    client.call.mockResolvedValueOnce(page(["a", "b"], "c1")).mockRejectedValueOnce(new RpcError("git-cursor-invalid")).mockResolvedValueOnce(page(["n", "a"], "c9"))
    await loadLog("work", "w1")
    await loadLog("work", "w1", true)
    expect(client.call.mock.calls.map(([, params]) => (params as { cursor?: string }).cursor)).toEqual([undefined, "c1", undefined])
    expect(log()?.commits.map((item) => item.hash)).toEqual(["n", "a"])
    expect(log()?.nextCursor).toBe("c9")
    expect(useCompanion.getState().notice).toMatchObject({ key: "git.historyReloaded" })
  })

  it("passes any other failure on to the view and keeps what is loaded", async () => {
    client.call.mockResolvedValueOnce(page(["a"], "c1")).mockRejectedValueOnce(new RpcError("git-failed"))
    await loadLog("work", "w1")
    await expect(loadLog("work", "w1", true)).rejects.toMatchObject({ code: "git-failed" })
    expect(log()?.commits).toHaveLength(1)
  })

  it("drops a page that arrives after the history was reloaded", async () => {
    let resolveMore: (value: GitLog) => void = () => {}
    client.call.mockResolvedValueOnce(page(["a"], "c1")).mockReturnValueOnce(new Promise((resolve) => { resolveMore = resolve })).mockResolvedValueOnce(page(["z"], "c7"))
    await loadLog("work", "w1")
    const more = loadLog("work", "w1", true)
    await loadLog("work", "w1")
    resolveMore(page(["b"], null))
    await more
    expect(log()?.commits.map((item) => item.hash)).toEqual(["z"])
  })

  it("forgets the loaded pages when the refresh button drops them", async () => {
    client.call.mockResolvedValueOnce(page(["a"], null))
    await loadLog("work", "w1")
    dropLog("work", "w1")
    expect(log()).toBeUndefined()
  })
})

describe("loadCommit", () => {
  const detail = { hash: "abc", subject: "s", body: "", authorName: "A", authorEmail: "a@x", timestamp: 1, parents: [], files: [], totalAdditions: 0, totalDeletions: 0, truncated: false } satisfies GitCommit

  it("reads a commit once: it never changes", async () => {
    client.call.mockResolvedValue(detail)
    await loadCommit("work", "w1", "abc")
    await loadCommit("work", "w1", "abc")
    expect(client.call).toHaveBeenCalledTimes(1)
    expect(client.call).toHaveBeenCalledWith("git.commit", { session: "work", workspaceId: "w1", hash: "abc" })
    expect(useCompanion.getState().git[spaceKey("work", "w1")].commits.abc).toEqual(detail)
  })
})

describe("loadDiff", () => {
  it("sends only what each kind of diff needs", async () => {
    client.call.mockResolvedValue({ text: "", truncated: false, binary: false })
    await loadDiff("work", "w1", { kind: "worktree", path: "a.ts", untracked: true })
    await loadDiff("work", "w1", { kind: "staged", path: "b.ts", origPath: "old.ts" })
    await loadDiff("work", "w1", { kind: "commit", hash: "abc", path: "c.ts", origPath: "old-c.ts" })
    expect(client.call.mock.calls).toEqual([
      ["git.diff", { session: "work", workspaceId: "w1", kind: "worktree", path: "a.ts", untracked: true }],
      ["git.diff", { session: "work", workspaceId: "w1", kind: "staged", path: "b.ts", origPath: "old.ts" }],
      ["git.diff", { session: "work", workspaceId: "w1", kind: "commit", path: "c.ts", origPath: "old-c.ts", commit: "abc" }],
    ])
  })

  it("keeps the screen-only conflicted flag away from the gateway", async () => {
    client.call.mockResolvedValue({ text: "", truncated: false, binary: false })
    await loadDiff("work", "w1", { kind: "worktree", path: "a.ts", untracked: false, conflicted: true })
    expect(client.call.mock.calls).toEqual([
      ["git.diff", { session: "work", workspaceId: "w1", kind: "worktree", path: "a.ts", untracked: false }],
    ])
  })
})

describe("a Space the desktop maps to another repository", () => {
  const sharing = (gitGeneration: string): Snapshot => ({
    session: "work",
    workspaces: [{ id: "w1", label: null, status: "idle", git: true, gitGeneration }],
    tabs: [],
    panes: [],
    agents: [],
  })

  it("tags what it reads with the repository it was read from", async () => {
    useCompanion.setState({ snapshots: { work: sharing("g1") } })
    client.call.mockResolvedValueOnce(page(["a"], null))
    await loadLog("work", "w1")
    expect(useCompanion.getState().git[spaceKey("work", "w1")]?.generation).toBe("g1")
  })

  it("does not file a read that was on its way during the remap under the new repository", async () => {
    useCompanion.setState({ snapshots: { work: sharing("g1") } })
    let finish: (value: GitLog) => void = () => {}
    client.call.mockReturnValueOnce(new Promise<GitLog>((resolve) => { finish = resolve }))
    const reading = loadLog("work", "w1")
    // The snapshot that names the new repository lands first.
    useCompanion.setState({ snapshots: { work: sharing("g2") } })
    finish(page(["old-repo-commit"], null))
    await reading
    expect(log()).toBeUndefined()
    // The next read, now of the new repository, is kept.
    client.call.mockResolvedValueOnce(page(["new-repo-commit"], null))
    await loadLog("work", "w1")
    expect(log()?.commits.map((item) => item.hash)).toEqual(["new-repo-commit"])
    expect(useCompanion.getState().git[spaceKey("work", "w1")]?.generation).toBe("g2")
  })

  it("applies the same rule to status", async () => {
    useCompanion.setState({ snapshots: { work: sharing("g1") } })
    let finish: (value: unknown) => void = () => {}
    client.call.mockReturnValueOnce(new Promise((resolve) => { finish = resolve }))
    const reading = loadStatus("work", "w1")
    useCompanion.setState({ snapshots: { work: sharing("g2") } })
    finish({ repo: { name: "old" }, status: {}, truncated: false })
    await reading
    expect(useCompanion.getState().git[spaceKey("work", "w1")]?.status).toBeUndefined()
  })
})

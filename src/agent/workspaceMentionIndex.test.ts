import { describe, expect, it, vi } from "vitest"

import {
  MAX_WORKSPACE_MENTION_RESULTS,
  WorkspaceMentionIndexCache,
  canonicalPathKey,
  rankWorkspaceMentions,
} from "@/agent/workspaceMentionIndex"
import type { WorkspacePathIndexResult } from "@/lib/types"

function result(
  entries: WorkspacePathIndexResult["entries"],
  workspace = "/workspace",
  truncated = false
): WorkspacePathIndexResult {
  return { workspace, entries, truncated }
}

function deferred<T>() {
  let resolve!: (value: T) => void
  let reject!: (error: unknown) => void
  const promise = new Promise<T>((res, rej) => {
    resolve = res
    reject = rej
  })
  return { promise, resolve, reject }
}

describe("workspaceMentionIndex", () => {
  it("normalizes portable canonical keys without making POSIX paths case-insensitive", () => {
    expect(canonicalPathKey("C:\\Work\\Repo\\")).toBe("c:/work/repo")
    expect(canonicalPathKey("C:/WORK/Repo")).toBe("c:/work/repo")
    expect(canonicalPathKey("\\\\?\\C:\\Work\\Repo")).toBe("c:/work/repo")
    expect(canonicalPathKey("\\\\?\\UNC\\Server\\Share\\Repo")).toBe("//server/share/repo")
    expect(canonicalPathKey("/Work/Repo/")).toBe("/Work/Repo")
    expect(canonicalPathKey("/Work/Repo")).not.toBe(canonicalPathKey("/work/repo"))
  })

  it("shares one in-flight request and caches by canonical workspace plus revision", async () => {
    const pending = deferred<WorkspacePathIndexResult>()
    const loader = vi.fn(() => pending.promise)
    const cache = new WorkspaceMentionIndexCache(loader)

    const first = cache.load("C:\\Work\\Repo", 4)
    const second = cache.load("c:/work/repo/", 4)
    expect(second).toBe(first)
    expect(loader).toHaveBeenCalledTimes(1)

    pending.resolve(result([
      { relativePath: "src/a.ts", canonicalPath: "C:\\Work\\Repo\\src\\a.ts" },
    ], "C:\\Work\\Repo"))
    await expect(first).resolves.toMatchObject({ revision: 4, truncated: false })

    await cache.load("C:/WORK/REPO", 4)
    expect(loader).toHaveBeenCalledTimes(1)
  })

  it("invalidates a reopened workspace across A→B→A while preserving same-activation reuse", async () => {
    const staleA = deferred<WorkspacePathIndexResult>()
    const freshA = deferred<WorkspacePathIndexResult>()
    const loader = vi.fn()
      .mockReturnValueOnce(staleA.promise)
      .mockReturnValueOnce(freshA.promise)
    const cache = new WorkspaceMentionIndexCache(loader)

    cache.activateWorkspace("/workspace-a")
    const staleLoad = cache.load("/workspace-a", 7)
    expect(cache.load("/workspace-a/", 7)).toBe(staleLoad)
    expect(loader).toHaveBeenCalledTimes(1)

    cache.activateWorkspace("/workspace-b")
    cache.activateWorkspace("/workspace-a")
    const freshLoad = cache.load("/workspace-a", 7)
    expect(freshLoad).not.toBe(staleLoad)
    expect(loader).toHaveBeenCalledTimes(2)

    staleA.resolve(result([
      { relativePath: "stale.ts", canonicalPath: "/workspace-a/stale.ts" },
    ], "/workspace-a"))
    await expect(staleLoad).resolves.toBeNull()

    freshA.resolve(result([
      { relativePath: "fresh.ts", canonicalPath: "/workspace-a/fresh.ts" },
    ], "/workspace-a"))
    await expect(freshLoad).resolves.toMatchObject({
      revision: 7,
      entries: [{ relativePath: "fresh.ts", canonicalPath: "/workspace-a/fresh.ts" }],
    })
    await expect(cache.load("/workspace-a", 7)).resolves.toMatchObject({ revision: 7 })
    expect(loader).toHaveBeenCalledTimes(2)
  })

  it("discards a late result after revision invalidation without overwriting the newer cache", async () => {
    const oldRequest = deferred<WorkspacePathIndexResult>()
    const newRequest = deferred<WorkspacePathIndexResult>()
    const loader = vi.fn()
      .mockReturnValueOnce(oldRequest.promise)
      .mockReturnValueOnce(newRequest.promise)
    const cache = new WorkspaceMentionIndexCache(loader)

    const oldLoad = cache.load("/workspace", 1)
    const newLoad = cache.load("/workspace", 2)
    oldRequest.resolve(result([
      { relativePath: "old.ts", canonicalPath: "/workspace/old.ts" },
    ]))
    await expect(oldLoad).resolves.toBeNull()

    newRequest.resolve(result([
      { relativePath: "new.ts", canonicalPath: "/workspace/new.ts" },
    ]))
    await expect(newLoad).resolves.toMatchObject({
      revision: 2,
      entries: [{ relativePath: "new.ts", canonicalPath: "/workspace/new.ts" }],
    })
    await expect(cache.load("/workspace", 2)).resolves.toMatchObject({ revision: 2 })
    expect(loader).toHaveBeenCalledTimes(2)
  })

  it("discards a late stale rejection instead of surfacing it into the current revision", async () => {
    const oldRequest = deferred<WorkspacePathIndexResult>()
    const newRequest = deferred<WorkspacePathIndexResult>()
    const loader = vi.fn()
      .mockReturnValueOnce(oldRequest.promise)
      .mockReturnValueOnce(newRequest.promise)
    const cache = new WorkspaceMentionIndexCache(loader)

    const oldLoad = cache.load("/workspace", 1)
    const newLoad = cache.load("/workspace", 2)
    oldRequest.reject(new Error("stale walk failed"))
    await expect(oldLoad).resolves.toBeNull()

    newRequest.resolve(result([]))
    await expect(newLoad).resolves.toMatchObject({ revision: 2, entries: [] })
  })

  it("fails closed on a mismatched workspace and filters escaped or duplicate entries", async () => {
    const mismatch = new WorkspaceMentionIndexCache(async () => result([], "/other"))
    await expect(mismatch.load("/workspace", 0)).rejects.toThrow("different canonical workspace")

    const cache = new WorkspaceMentionIndexCache(async () => result([
      { relativePath: "../escape.ts", canonicalPath: "/outside/escape.ts" },
      { relativePath: "src/a.ts", canonicalPath: "/workspace/src/a.ts" },
      { relativePath: "src/copy.ts", canonicalPath: "/workspace/src/a.ts" },
      { relativePath: "/absolute.ts", canonicalPath: "/workspace/absolute.ts" },
    ]))
    await expect(cache.load("/workspace", 0)).resolves.toMatchObject({
      entries: [{ relativePath: "src/a.ts", canonicalPath: "/workspace/src/a.ts" }],
    })
  })

  it("ranks fuzzy matches deterministically and never renders more than the top 100", () => {
    const entries = Array.from({ length: 140 }, (_, index) => ({
      relativePath: `src/group-${String(index).padStart(3, "0")}/button.tsx`,
      canonicalPath: `/workspace/src/group-${String(index).padStart(3, "0")}/button.tsx`,
    }))
    entries.push(
      { relativePath: "button.tsx", canonicalPath: "/workspace/button.tsx" },
      { relativePath: "src/better-button.tsx", canonicalPath: "/workspace/src/better-button.tsx" }
    )

    const ranked = rankWorkspaceMentions(entries, "btn", 999)
    expect(ranked).toHaveLength(MAX_WORKSPACE_MENTION_RESULTS)
    expect(ranked[0].relativePath).toBe("button.tsx")
    expect(ranked.map((entry) => entry.canonicalPath).length).toBe(
      new Set(ranked.map((entry) => entry.canonicalPath)).size
    )
    expect(rankWorkspaceMentions(entries, "button", 2).map((entry) => entry.relativePath)).toEqual([
      "button.tsx",
      "src/group-000/button.tsx",
    ])
  })

  it("can retry after a loader error and explicit invalidation", async () => {
    const loader = vi.fn()
      .mockRejectedValueOnce(new Error("walk failed"))
      .mockResolvedValue(result([]))
    const cache = new WorkspaceMentionIndexCache(loader)

    await expect(cache.load("/workspace", 0)).rejects.toThrow("walk failed")
    await expect(cache.load("/workspace", 0)).resolves.toMatchObject({ entries: [] })
    cache.invalidate("/workspace")
    await cache.load("/workspace", 0)
    expect(loader).toHaveBeenCalledTimes(3)
  })
})

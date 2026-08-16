import { describe, expect, it } from "vitest"

import type { HerdrSpaceInfo, HerdrWorktreeListResult } from "./herdrTypes"
import {
  applyWorktreeEntryToSpace,
  buildWorktreeInventory,
  mergeSpaceWorktreeProvenance,
  spaceProvenanceFromSnapshotWorktree
} from "./herdrWorktree"

function space(partial: Partial<HerdrSpaceInfo> & Pick<HerdrSpaceInfo, "id">): HerdrSpaceInfo {
  const { id, label, order, focused, path, status, ...rest } = partial
  return {
    id,
    label: label ?? id,
    order: order ?? 0,
    focused: focused ?? false,
    path: path ?? null,
    status: status ?? "idle",
    ...rest
  }
}

const sourceList: HerdrWorktreeListResult = {
  source: {
    repoKey: "/Users/me/yuzora/.git",
    repoName: "yuzora",
    repoRoot: "/Users/me/yuzora",
    sourceCheckoutPath: "/Users/me/yuzora",
    sourceWorkspaceId: "ws-source"
  },
  worktrees: [
    {
      path: "/Users/me/yuzora",
      branch: "main",
      isBare: false,
      isDetached: false,
      isPrunable: false,
      isLinkedWorktree: false,
      label: "yuzora",
      openWorkspaceId: "ws-source"
    },
    {
      path: "/Users/me/yuzora-feature",
      branch: "feature/x",
      isBare: false,
      isDetached: false,
      isPrunable: false,
      isLinkedWorktree: true,
      label: "feature-x",
      openWorkspaceId: "ws-linked"
    },
    {
      path: "/Users/me/yuzora-detached",
      branch: null,
      isBare: false,
      isDetached: true,
      isPrunable: true,
      isLinkedWorktree: true,
      label: "detached",
      openWorkspaceId: "ws-detached"
    },
    {
      path: "/Users/me/yuzora-closed",
      branch: "orphan",
      isBare: false,
      isDetached: false,
      isPrunable: false,
      isLinkedWorktree: true,
      label: "closed",
      openWorkspaceId: null
    }
  ]
}

describe("buildWorktreeInventory", () => {
  it("indexes only by open_workspace_id and never by path", () => {
    const inventory = buildWorktreeInventory("work", [sourceList])
    expect(inventory.sessionName).toBe("work")
    expect(Object.keys(inventory.byOpenWorkspaceId).sort()).toEqual([
      "ws-detached",
      "ws-linked",
      "ws-source"
    ])
    expect(inventory.byOpenWorkspaceId["ws-linked"]?.worktree.path).toBe(
      "/Users/me/yuzora-feature"
    )
    // Closed worktree without open workspace is retained in lists, not index.
    expect(inventory.lists[0]?.worktrees).toHaveLength(4)
  })

  it("does not expose prototype members as workspace inventory entries", () => {
    const inventory = buildWorktreeInventory("work", [])
    const spaces = ["constructor", "toString", "__proto__"].map((id) =>
      space({ id, path: `/safe/${id}` })
    )
    expect(mergeSpaceWorktreeProvenance(spaces, inventory)).toEqual(spaces)
  })
})

describe("mergeSpaceWorktreeProvenance", () => {
  it("merges source checkout vs linked worktree and detached/null branch by workspace id", () => {
    const spaces = [
      space({
        id: "ws-source",
        label: "Yuzora",
        path: "/Users/me/yuzora-from-snapshot"
      }),
      space({
        id: "ws-linked",
        label: "Feature",
        path: "/Users/me/yuzora-feature-from-snapshot",
        isLinkedWorktree: false
      }),
      space({
        id: "ws-detached",
        label: "Detached",
        path: "/Users/me/yuzora-detached"
      }),
      space({
        id: "ws-plain",
        label: "Folder",
        path: "/Users/me/notes"
      })
    ]
    const inventory = buildWorktreeInventory("default", [sourceList])
    const merged = mergeSpaceWorktreeProvenance(spaces, inventory)

    expect(merged.find((s) => s.id === "ws-source")).toEqual(
      expect.objectContaining({
        id: "ws-source",
        path: "/Users/me/yuzora",
        repoName: "yuzora",
        repoRoot: "/Users/me/yuzora",
        sourceCheckoutPath: "/Users/me/yuzora",
        branch: "main",
        isLinkedWorktree: false,
        isDetached: false,
        isPrunable: false
      })
    )
    expect(merged.find((s) => s.id === "ws-linked")).toEqual(
      expect.objectContaining({
        id: "ws-linked",
        path: "/Users/me/yuzora-feature",
        branch: "feature/x",
        isLinkedWorktree: true,
        isDetached: false
      })
    )
    expect(merged.find((s) => s.id === "ws-detached")).toEqual(
      expect.objectContaining({
        id: "ws-detached",
        branch: null,
        isDetached: true,
        isPrunable: true,
        isLinkedWorktree: true
      })
    )
    // Unmatched plain folder Space is unchanged — no path-based guessing.
    expect(merged.find((s) => s.id === "ws-plain")).toEqual(spaces[3])
  })

  it("preserves Windows drive/UNC/verbatim checkout paths as operational identity", () => {
    const windowsList: HerdrWorktreeListResult = {
      source: {
        repoKey: String.raw`C:\src\yuzora\.git`,
        repoName: "yuzora",
        repoRoot: String.raw`C:\src\yuzora`,
        sourceCheckoutPath: String.raw`\\?\C:\src\yuzora`,
        sourceWorkspaceId: "w1"
      },
      worktrees: [
        {
          path: String.raw`\\?\C:\src\yuzora`,
          branch: "main",
          isBare: false,
          isDetached: false,
          isPrunable: false,
          isLinkedWorktree: false,
          label: "main",
          openWorkspaceId: "w1"
        },
        {
          path: String.raw`\\server\share\yuzora-wt`,
          branch: "feat",
          isBare: false,
          isDetached: false,
          isPrunable: false,
          isLinkedWorktree: true,
          label: "unc-wt",
          openWorkspaceId: "w2"
        },
        {
          path: String.raw`D:\worktrees\feature`,
          branch: null,
          isBare: false,
          isDetached: true,
          isPrunable: false,
          isLinkedWorktree: true,
          label: "drive-wt",
          openWorkspaceId: "w3"
        }
      ]
    }
    const spaces = [
      space({ id: "w1", path: String.raw`C:\src\yuzora` }),
      space({ id: "w2", path: null }),
      space({ id: "w3", path: null })
    ]
    const merged = mergeSpaceWorktreeProvenance(
      spaces,
      buildWorktreeInventory("win", [windowsList])
    )
    expect(merged[0]?.path).toBe(String.raw`\\?\C:\src\yuzora`)
    expect(merged[0]?.sourceCheckoutPath).toBe(String.raw`\\?\C:\src\yuzora`)
    expect(merged[0]?.repoRoot).toBe(String.raw`C:\src\yuzora`)
    expect(merged[1]?.path).toBe(String.raw`\\server\share\yuzora-wt`)
    expect(merged[2]?.path).toBe(String.raw`D:\worktrees\feature`)
    expect(merged[2]?.branch).toBeNull()
    expect(merged[2]?.isDetached).toBe(true)
  })

  it("leaves spaces untouched when inventory is null", () => {
    const spaces = [space({ id: "ws-1", path: "/a" })]
    expect(mergeSpaceWorktreeProvenance(spaces, null)).toBe(spaces)
  })
})

describe("applyWorktreeEntryToSpace / snapshot provenance", () => {
  it("keeps existing path when inventory path is empty", () => {
    const next = applyWorktreeEntryToSpace(
      space({ id: "ws-1", path: "/existing" }),
      {
        path: "",
        branch: "main",
        isBare: false,
        isDetached: false,
        isPrunable: false,
        isLinkedWorktree: false,
        label: "x",
        openWorkspaceId: "ws-1"
      },
      {
        repoKey: "k",
        repoName: "r",
        repoRoot: "/r",
        sourceCheckoutPath: "/r"
      }
    )
    expect(next.path).toBe("/existing")
    expect(next.branch).toBe("main")
  })

  it("extracts WorkspaceWorktreeInfo fields without inventing branch", () => {
    expect(
      spaceProvenanceFromSnapshotWorktree({
        checkout_path: "/Users/me/yuzora",
        is_linked_worktree: true,
        repo_key: "k",
        repo_name: "yuzora",
        repo_root: "/Users/me/yuzora"
      })
    ).toEqual({
      path: "/Users/me/yuzora",
      repoKey: "k",
      repoName: "yuzora",
      repoRoot: "/Users/me/yuzora",
      isLinkedWorktree: true
    })
  })
})

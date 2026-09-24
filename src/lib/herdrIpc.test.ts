import { afterEach, describe, expect, it } from "vitest"
import { clearMocks, mockIPC } from "@tauri-apps/api/mocks"

import {
  herdrBinarySourceGet,
  herdrBinarySourceSet,
  herdrEventsRelease,
  herdrLayoutExport,
  herdrLayoutSetSplitRatio,
  herdrPaneClose,
  herdrPaneRename,
  herdrPaneSplit,
  herdrPaneSwap,
  herdrPaneZoom,
  herdrTabClose,
  herdrTabCreate,
  herdrTabMove,
  herdrTabRename,
  herdrWorkspaceClose,
  herdrWorkspaceMove,
  herdrWorkspaceRename,
  herdrWorktreeList
} from "./herdrIpc"

afterEach(() => {
  clearMocks()
})

describe("herdrIpc native interaction wrappers", () => {
  it("invokes binary-source and event-release commands exactly", async () => {
    const calls: Array<{ cmd: string; args: Record<string, unknown> }> = []
    mockIPC((cmd, args) => {
      calls.push({ cmd, args: (args ?? {}) as Record<string, unknown> })
      if (cmd === "herdr_binary_source_get") {
        return {
          configured: "global",
          active: "global",
          resolved: "global",
          available: true,
          configuredAvailable: true,
          restartRequired: false
        }
      }
      if (cmd === "herdr_binary_source_set") {
        return { configured: "default", restartRequired: true }
      }
      return null
    })

    await herdrBinarySourceGet()
    await herdrBinarySourceSet("default")
    await herdrEventsRelease("sub-1")

    expect(calls).toEqual([
      { cmd: "herdr_binary_source_get", args: {} },
      { cmd: "herdr_binary_source_set", args: { source: "default", customPath: null } },
      { cmd: "herdr_events_release", args: { subscriptionId: "sub-1" } }
    ])
  })

  it("invokes workspace/tab/pane/layout commands with camelCase payloads", async () => {
    const calls: Array<{ cmd: string; args: Record<string, unknown> }> = []
    mockIPC((cmd, args) => {
      calls.push({ cmd, args: (args ?? {}) as Record<string, unknown> })
      if (cmd === "herdr_tab_create") {
        return {
          terminalId: "t1",
          paneId: "p1",
          tabId: "tab1",
          workspaceId: "ws1",
          title: "Tab"
        }
      }
      if (cmd === "herdr_pane_split") {
        return {
          paneId: "p2",
          terminalId: "t2",
          tabId: "tab1",
          workspaceId: "ws1",
          title: null
        }
      }
      if (cmd === "herdr_layout_export" || cmd === "herdr_layout_set_split_ratio") {
        return {
          workspaceId: "ws1",
          tabId: "tab1",
          zoomed: false,
          focusedPaneId: "p1",
          root: { type: "pane", paneId: "p1", label: null, cwd: null }
        }
      }
      return null
    })

    await herdrWorkspaceRename({
      sessionName: "default",
      workspaceId: "ws1",
      label: "Renamed"
    })
    await herdrWorkspaceClose({ sessionName: "default", workspaceId: "ws1" })
    await herdrWorkspaceMove({ sessionName: "default", workspaceId: "ws1", insertIndex: 2 })
    await herdrTabCreate({ sessionName: "default", workspaceId: "ws1", focus: true })
    await herdrTabRename({ sessionName: "default", tabId: "tab1", label: "Main" })
    await herdrTabClose({ sessionName: "default", tabId: "tab1" })
    await herdrTabMove({ sessionName: "default", tabId: "tab1", insertIndex: 1 })
    await herdrPaneRename({ sessionName: "default", paneId: "p1", label: "Shell" })
    await herdrPaneSplit({
      sessionName: "default",
      direction: "right",
      targetPaneId: "p1"
    })
    await herdrPaneZoom({ sessionName: "default", paneId: "p1", mode: "toggle" })
    await herdrPaneSwap({
      sessionName: "default",
      sourcePaneId: "p1",
      targetPaneId: "p2"
    })
    await herdrPaneClose({ sessionName: "default", paneId: "p1" })
    await herdrLayoutExport({ sessionName: "default", tabId: "tab1" })
    await herdrLayoutSetSplitRatio({
      sessionName: "default",
      tabId: "tab1",
      path: [false, true],
      ratio: 0.4
    })

    expect(calls.map((c) => c.cmd)).toEqual([
      "herdr_workspace_rename",
      "herdr_workspace_close",
      "herdr_workspace_move",
      "herdr_tab_create",
      "herdr_tab_rename",
      "herdr_tab_close",
      "herdr_tab_move",
      "herdr_pane_rename",
      "herdr_pane_split",
      "herdr_pane_zoom",
      "herdr_pane_swap",
      "herdr_pane_close",
      "herdr_layout_export",
      "herdr_layout_set_split_ratio"
    ])
    expect(calls.find((c) => c.cmd === "herdr_layout_set_split_ratio")?.args).toMatchObject({
      path: [false, true],
      ratio: 0.4,
      tabId: "tab1"
    })
    expect(calls.find((c) => c.cmd === "herdr_tab_move")).toEqual({
      cmd: "herdr_tab_move",
      args: {
        sessionName: "default",
        tabId: "tab1",
        insertIndex: 1
      }
    })
    expect(calls.find((c) => c.cmd === "herdr_workspace_move")).toEqual({
      cmd: "herdr_workspace_move",
      args: {
        sessionName: "default",
        workspaceId: "ws1",
        insertIndex: 2
      }
    })
    expect(calls.find((c) => c.cmd === "herdr_pane_split")?.args).toMatchObject({
      direction: "right",
      targetPaneId: "p1"
    })
  })
})

describe("herdrWorktreeList", () => {
  it("invokes herdr_worktree_list with named session and optional filters", async () => {
    const calls: Array<{ cmd: string; args: Record<string, unknown> }> = []
    mockIPC((cmd, args) => {
      calls.push({ cmd, args: (args ?? {}) as Record<string, unknown> })
      return {
        source: {
          repoKey: "/r/.git",
          repoName: "r",
          repoRoot: "/r",
          sourceCheckoutPath: "/r",
          sourceWorkspaceId: "w1"
        },
        worktrees: []
      }
    })
    await herdrWorktreeList({ sessionName: "work", workspaceId: "w1", cwd: null })
    expect(calls).toEqual([
      {
        cmd: "herdr_worktree_list",
        args: { sessionName: "work", cwd: null, workspaceId: "w1" }
      }
    ])
  })
})

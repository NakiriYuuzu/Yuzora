import { beforeEach, describe, expect, it } from "vitest"

import { herdrAttachmentKey } from "@/lib/herdrPages"
import { useWorkspaceStore } from "@/state/workspaceStore"

describe("workspace Herdr runtime identity", () => {
  beforeEach(() => {
    useWorkspaceStore.setState({
      groups: [{ id: "group-1", tabs: [], activePath: null }],
      activeGroupIndex: 0
    })
  })

  it("migrates legacy native pages idempotently without changing their path", () => {
    const path = "yuzora://herdr/default/term-legacy"
    useWorkspaceStore.setState({
      groups: [{
        id: "group-1",
        activePath: path,
        tabs: [{
          path,
          name: "Legacy",
          dirty: false,
          externallyModified: false,
          kind: "herdr-terminal",
          herdrSessionId: "default",
          terminalId: "term-legacy",
          herdrTabId: "tab-legacy",
          herdrWorkspaceId: "ws-legacy"
        }]
      }]
    })
    const snapshot = {
      herdrSessionId: "default",
      runtimeTarget: { kind: "native" as const },
      protocol: 19,
      version: "0.8.0",
      spaces: [{ id: "ws-legacy", label: "Legacy", order: 0, focused: true }],
      agents: [],
      tabs: [{ id: "tab-legacy", label: "Legacy", order: 0, workspaceId: "ws-legacy", paneCount: 1, status: "idle" as const, active: true, focused: true, terminalId: "term-legacy", sessionName: "default" }],
      terminals: [],
      focusedWorkspaceId: "ws-legacy",
      focusedTabId: "tab-legacy",
      focusedPaneId: null,
      raw: {}
    }

    useWorkspaceStore.getState().reconcileHerdrPagesFromSnapshot(snapshot, "default")
    useWorkspaceStore.getState().reconcileHerdrPagesFromSnapshot(snapshot, "default")
    const tab = useWorkspaceStore.getState().groups[0]!.tabs[0]!
    expect(tab.path).toBe(path)
    expect(tab.herdrRuntimeTarget).toEqual({ kind: "native" })
  })

  it("retains an inactive missing-distro page rather than reinterpreting it as Native", () => {
    const path = "yuzora://herdr/v2/wsl%3AMissing%20Distro/default/term-1"
    useWorkspaceStore.setState({
      groups: [{
        id: "group-1",
        activePath: null,
        tabs: [{
          path,
          name: "Missing WSL",
          dirty: false,
          externallyModified: false,
          kind: "herdr-terminal",
          herdrSessionId: "default",
          herdrRuntimeTarget: { kind: "wsl", distro: "Missing Distro" },
          terminalId: "term-1",
          herdrTabId: "tab-1",
          herdrWorkspaceId: "ws-1"
        }]
      }]
    })
    const nativeSnapshot = {
      herdrSessionId: "default",
      runtimeTarget: { kind: "native" as const },
      protocol: 19,
      version: "0.8.0",
      spaces: [], agents: [], tabs: [], terminals: [],
      focusedWorkspaceId: null, focusedTabId: null, focusedPaneId: null, raw: {}
    }
    useWorkspaceStore.getState().reconcileHerdrPagesFromSnapshot(nativeSnapshot, "default")
    const tab = useWorkspaceStore.getState().groups[0]!.tabs[0]!
    expect(tab.path).toBe(path)
    expect(tab.herdrRuntimeTarget).toEqual({ kind: "wsl", distro: "Missing Distro" })
  })

  it("does not dedupe same session/tab/terminal IDs across Runtime Environments", () => {
    useWorkspaceStore.getState().openHerdrTerminalPage({
      herdrSessionId: "default",
      runtimeTarget: { kind: "native" },
      terminalId: "term-1",
      herdrTabId: "tab-1",
      herdrWorkspaceId: "workspace-1"
    })
    useWorkspaceStore.getState().openHerdrTerminalPage({
      herdrSessionId: "default",
      runtimeTarget: { kind: "wsl", distro: "Ubuntu" },
      terminalId: "term-1",
      herdrTabId: "tab-1",
      herdrWorkspaceId: "workspace-1"
    })

    const tabs = useWorkspaceStore.getState().groups[0]!.tabs
    expect(tabs).toHaveLength(2)
    expect(tabs.map((tab) => tab.path)).toEqual([
      "yuzora://herdr/default/term-1",
      "yuzora://herdr/v2/wsl%3AUbuntu/default/term-1"
    ])
    expect(herdrAttachmentKey(tabs[0]!.path, "pane-1")).not.toBe(
      herdrAttachmentKey(tabs[1]!.path, "pane-1")
    )
  })
})

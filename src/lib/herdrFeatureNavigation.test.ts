import { beforeEach, expect, it, vi } from "vitest"
import { closeRemovedHerdrPages, activateHerdrFeatureResult } from "./herdrFeatureNavigation"
import { useWorkspaceStore } from "@/state/workspaceStore"
import { herdrInitialState, useHerdrStore } from "@/state/herdrStore"
import { herdrPagePath } from "./herdrPages"
import { runtimeKey } from "./runtimeIdentity"
import type { HerdrSnapshot, HerdrTabInfo } from "./herdrTypes"

const remote = runtimeKey({ hostId: "other-host", sessionName: "default" })
const page = (scope: string, pane: string, tab: string) => ({ path: herdrPagePath(scope, pane), name: pane, kind: "herdr-terminal" as const, herdrSessionId: scope, herdrTabId: tab, herdrWorkspaceId: "w1", terminalId: pane, paneId: pane, dirty: false, externallyModified: false })
beforeEach(() => {
  useHerdrStore.setState({ ...herdrInitialState, sessions: [{ name: "default", default: true, running: true, sessionDir: "/s", socketPath: "/s/socket" }] })
  useWorkspaceStore.setState({ groups: [{ tabs: [page("default", "p1", "old"), page("default", "p2", "survives"), page(remote, "p1", "old"), { path: "/dirty.txt", name: "dirty", dirty: true, externallyModified: false }], activePath: herdrPagePath("default", "p1") }], activeGroupIndex: 0, dismissedHerdrPages: {} })
})
it("closes only the moved pane's removed source tab, preserving other hosts and dirty editors", () => {
  closeRemovedHerdrPages("default", { method: "pane.move", params: { pane_id: "p1", destination: { type: "new_workspace" }, focus: false } }, { move_result: { closed_tab_id: "old", pane: { pane_id: "new:p1", tab_id: "new" } } })
  expect(useWorkspaceStore.getState().groups[0].tabs.map(tab => tab.path)).toEqual([herdrPagePath("default", "p2"), herdrPagePath(remote, "p1"), "/dirty.txt"])
})
it("activates the new tab using its refreshed identity instead of the moved pane's stale id", async () => {
  const activate = vi.spyOn(useHerdrStore.getState(), "activateTab").mockResolvedValue({ ok: true })
  const tab: HerdrTabInfo = { id: "new", workspaceId: "w2", terminalId: "new:p1", label: "Moved", order: 0, paneCount: 1, status: "idle", active: true, focused: true }
  useHerdrStore.setState({ runtimesBySession: { default: { snapshot: { tabs: [tab] } as HerdrSnapshot, capabilities: null, worktreeInventory: null, connectionState: "ready", errorMessage: null } } })
  try {
    await activateHerdrFeatureResult("default", { method: "pane.move", params: { pane_id: "p1", destination: { type: "new_workspace" }, focus: false } }, { move_result: { pane: { pane_id: "new:p1", tab_id: "new" } } })
    expect(activate).toHaveBeenCalledExactlyOnceWith({ ...tab, sessionName: "default" })
  } finally { activate.mockRestore() }
})

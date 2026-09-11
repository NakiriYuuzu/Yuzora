import { beforeEach, expect, it } from "vitest"
import { useWorkspaceStore } from "./workspaceStore"
const initial = useWorkspaceStore.getState()
beforeEach(() => useWorkspaceStore.setState(initial, true))
it("reopening the same HERDR page 100 times does not notify subscribers", () => {
  const args = { herdrSessionId: "default", terminalId: "t", herdrTabId: "tab", herdrWorkspaceId: "space", paneId: "p", title: "Terminal" }
  useWorkspaceStore.getState().openHerdrTerminalPage(args)
  let notifications = 0
  const unsubscribe = useWorkspaceStore.subscribe(() => notifications++)
  for (let i = 0; i < 100; i++) useWorkspaceStore.getState().openHerdrTerminalPage(args)
  unsubscribe()
  expect(notifications).toBe(0)
})
it("selecting the active page 100 times does not notify subscribers", () => {
  useWorkspaceStore.getState().openTab("/repo/file.ts")
  let notifications = 0
  const unsubscribe = useWorkspaceStore.subscribe(() => notifications++)
  for (let i = 0; i < 100; i++) useWorkspaceStore.getState().setActiveTab(0, "/repo/file.ts")
  unsubscribe()
  expect(notifications).toBe(0)
})

it("restores editor group selection per host/session/Space and ignores closed or foreign-workspace pages", () => {
  const state = useWorkspaceStore.getState()
  state.setWorkspace("/repo")
  state.openTab("/repo/a.ts")
  state.splitRight()
  state.openTab("/repo/b.ts", 1)
  state.rememberSpaceNavigation('["host-a","work"]', "space")
  state.setActiveTab(0, "/repo/a.ts")
  state.rememberSpaceNavigation('["host-b","work"]', "space")
  expect(state.restoreSpaceNavigation('["host-a","work"]', "space")).toBe(true)
  expect(useWorkspaceStore.getState().activeGroupIndex).toBe(1)
  expect(state.restoreSpaceNavigation('["host-b","work"]', "space")).toBe(true)
  expect(useWorkspaceStore.getState().activeGroupIndex).toBe(0)
  expect(state.restoreSpaceNavigation('["host-a","other"]', "space")).toBe(false)
  state.setWorkspace("/other")
  expect(state.restoreSpaceNavigation('["host-a","work"]', "space")).toBe(false)
})

it("restores the last editor split after real workspace replacement and flat session reopening", () => {
  const state = useWorkspaceStore.getState()
  state.setWorkspace("/a")
  state.openTab("/a/left.ts")
  state.splitRight()
  state.openTab("/a/right.ts", 1)
  state.rememberSpaceNavigation("default", "a")
  state.setWorkspace("/b")
  state.setWorkspace("/a")
  // workspaceActions restores the persisted file inventory into group zero.
  state.openTab("/a/left.ts", 0)
  state.openTab("/a/right.ts", 0)
  expect(state.restoreSpaceNavigation("default", "a")).toBe(true)
  const restored = useWorkspaceStore.getState()
  expect(restored.activeGroupIndex).toBe(1)
  expect(restored.groups[1].activePath).toBe("/a/right.ts")
  expect(restored.groups[0].tabs.map(tab => tab.path)).not.toContain("/a/right.ts")
})

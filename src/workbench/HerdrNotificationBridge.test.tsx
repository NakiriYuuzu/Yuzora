import { act, cleanup, render } from "@testing-library/react"
import { afterEach, beforeEach, expect, it, vi } from "vitest"
import { HerdrNotificationBridge } from "./HerdrNotificationBridge"
import { herdrInitialState, useHerdrStore } from "@/state/herdrStore"
import { useHerdrNotificationStore } from "@/state/herdrNotificationStore"
import { useWorkspaceStore } from "@/state/workspaceStore"
import { useHerdrNativeStore } from "@/state/herdrNativeStore"
import type { HerdrAttentionItem, HerdrSessionRuntime, HerdrSnapshot } from "@/lib/herdrTypes"

const mocks = vi.hoisted(() => ({ toast: vi.fn(), permission: vi.fn(), system: vi.fn() }))
vi.mock("sonner", () => ({ toast: mocks.toast }))
vi.mock("@tauri-apps/plugin-notification", () => ({ isPermissionGranted: mocks.permission, sendNotification: mocks.system }))
const attention: HerdrAttentionItem = { key: "default:p1", sessionName: "default", paneId: "p1", kind: "done", agentStatus: "done", seen: false, updatedAt: 1 }
const runtime = (connectionState: HerdrSessionRuntime["connectionState"]): HerdrSessionRuntime => ({ connectionState, snapshot: { focusedPaneId: "p1", agents: [{ paneId: "p1", tabId: "t1" }] } as HerdrSnapshot, capabilities: null, worktreeInventory: null, errorMessage: null })
beforeEach(() => {
  vi.resetAllMocks(); mocks.permission.mockResolvedValue(false)
  vi.spyOn(document, "hasFocus").mockReturnValue(false)
  useHerdrStore.setState({ ...herdrInitialState, attentionByKey: new Map(), runtimesBySession: { default: runtime("ready") } })
  useHerdrNotificationStore.setState({ toast: true, system: true, sound: false, done: true, blocked: true })
  useWorkspaceStore.setState({ groups: [], activeGroupIndex: 0 })
  useHerdrNativeStore.setState({ selection: null })
})
afterEach(() => { cleanup(); vi.restoreAllMocks() })
async function emit(item = attention) {
  await act(async () => { useHerdrStore.setState({ attentionByKey: new Map([[item.key, item]]) }) })
}
it("does not replay historical completions from the initial or reconnect snapshot", async () => {
  useHerdrStore.setState({ runtimesBySession: { default: runtime("idle") } })
  render(<HerdrNotificationBridge />)
  await act(async () => { useHerdrStore.setState({ runtimesBySession: { default: runtime("ready") }, attentionByKey: new Map([[attention.key, attention]]) }) })
  expect(mocks.toast).not.toHaveBeenCalled()
  expect(mocks.permission).not.toHaveBeenCalled()
})
it("delivers one notification per new background state and respects switches", async () => {
  render(<HerdrNotificationBridge />)
  await emit(); await emit({ ...attention, updatedAt: 2 })
  expect(mocks.toast).toHaveBeenCalledTimes(1)
  useHerdrNotificationStore.setState({ blocked: false })
  await emit({ ...attention, kind: "blocked" })
  expect(mocks.toast).toHaveBeenCalledTimes(1)
})
it("silently reconciles a recovery snapshot while ready, then notifies on the next live transition", async () => {
  render(<HerdrNotificationBridge />)
  await act(async () => {
    useHerdrStore.getState().setEventsHealth("default", false)
    useHerdrStore.getState().applySnapshot("default", {
      herdrSessionId: "default", protocol: 22, version: "0.9.1", raw: {},
      spaces: [], tabs: [], terminals: [], focusedPaneId: "p1",
      agents: [{ id: "a1", name: "Agent", paneId: "p1", tabId: "t1", workspaceId: "w1", status: "done" }]
    })
  })
  expect([...useHerdrStore.getState().attentionByKey.values()]).toMatchObject([{ paneId: "p1", kind: "done", seen: false }])
  expect(mocks.toast).not.toHaveBeenCalled()
  expect(mocks.permission).not.toHaveBeenCalled()
  await act(async () => {
    useHerdrStore.getState().setEventsHealth("default", true, "new-subscription")
    for (const agentStatus of ["working", "done"] as const) {
      useHerdrStore.getState().applySubscriptionEvent("default", {
        type: "agent_status_changed", subscriptionId: "new-subscription", paneId: "p1", workspaceId: "w1", agentStatus, stateLabels: {}
      })
    }
  })
  expect(mocks.toast).toHaveBeenCalledTimes(1)
})
it("suppresses the visible pane in the native Session view", async () => {
  vi.mocked(document.hasFocus).mockReturnValue(true)
  useHerdrNativeStore.setState({ selection: { sessionName: "default" } })
  render(<HerdrNotificationBridge />); await emit()
  expect(mocks.toast).not.toHaveBeenCalled()
})
it("drops a delayed system notification if its attention has already cleared", async () => {
  let finish!: (value: boolean) => void
  mocks.permission.mockReturnValue(new Promise(resolve => { finish = resolve }))
  render(<HerdrNotificationBridge />); await emit()
  await act(async () => { useHerdrStore.setState({ attentionByKey: new Map() }); finish(true) })
  expect(mocks.system).not.toHaveBeenCalled()
})

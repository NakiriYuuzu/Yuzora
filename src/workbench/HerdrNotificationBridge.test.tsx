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

it.each([
  ["done", "category", 0], ["blocked", "category", 0],
  ["done", "other-category", 1], ["blocked", "other-category", 1],
  ["done", "system", 0], ["blocked", "system", 0],
  ["done", "unchanged", 1], ["blocked", "unchanged", 1],
] as const)("rechecks pending %s notification preferences after %s changes", async (kind, change, deliveries) => {
  let finish!: (value: boolean) => void
  mocks.permission.mockReturnValue(new Promise<boolean>(resolve => { finish = resolve }))
  useHerdrNotificationStore.setState({ toast: false })
  render(<HerdrNotificationBridge />)
  await emit({ ...attention, kind, agentStatus: kind })
  expect(mocks.permission).toHaveBeenCalledOnce()
  if (change === "category") useHerdrNotificationStore.setState({ [kind]: false })
  else if (change === "other-category") useHerdrNotificationStore.setState({ [kind === "done" ? "blocked" : "done"]: false })
  else if (change === "system") useHerdrNotificationStore.setState({ system: false })
  await act(async () => { finish(true) })
  expect(mocks.system).toHaveBeenCalledTimes(deliveries)
  expect(mocks.toast).not.toHaveBeenCalled()
})

it.each(["blocked", "unknown"] as const)("notifies a live completion after previously seen %s exactly once", async previousStatus => {
  useHerdrStore.getState().setEventsHealth("default", true, "sub-default")
  const emitStatus = (agentStatus: string) => useHerdrStore.getState().applySubscriptionEvent("default", {
    type: "agent_status_changed", subscriptionId: "sub-default", paneId: "p1", workspaceId: "w1", agentStatus, stateLabels: {}
  })
  emitStatus(previousStatus)
  useHerdrStore.getState().markAttentionSeen("default", "p1")
  useHerdrStore.setState({ selectedSessionName: "other" })
  mocks.permission.mockResolvedValue(true)
  render(<HerdrNotificationBridge />)
  await act(async () => { emitStatus("done") })
  expect(mocks.toast).toHaveBeenCalledTimes(1)
  expect(mocks.system).toHaveBeenCalledTimes(1)
  expect(useHerdrStore.getState().attentionItems("default")).toMatchObject([{ kind: "done", seen: false }])
  await act(async () => {
    emitStatus("done")
    useHerdrStore.getState().applySnapshot("default", {
      herdrSessionId: "default", protocol: 22, version: "0.9.1", raw: {},
      spaces: [], tabs: [], terminals: [],
      agents: [{ id: "a1", name: "Agent", paneId: "p1", tabId: "t1", workspaceId: "w1", status: "done" }]
    })
  })
  expect(mocks.toast).toHaveBeenCalledTimes(1)
  expect(mocks.system).toHaveBeenCalledTimes(1)
  expect(useHerdrStore.getState().attentionItems("default")).toHaveLength(1)
  await act(async () => {
    useHerdrStore.getState().markAttentionSeen("default", "p1")
    emitStatus("done")
  })
  expect(mocks.toast).toHaveBeenCalledTimes(1)
  expect(mocks.system).toHaveBeenCalledTimes(1)
  expect(useHerdrStore.getState().attentionItems("default")).toEqual([])
})

it.each(["blocked", "unknown"] as const)("silently reconciles seen %s into an unread completion from a snapshot", async status => {
  const snapshot: HerdrSnapshot = {
    herdrSessionId: "default", protocol: 22, version: "0.9.1", raw: {},
    spaces: [], tabs: [], terminals: [],
    agents: [{ id: "a1", name: "Agent", paneId: "p1", workspaceId: "w1", status }]
  }
  useHerdrStore.getState().applySnapshot("default", snapshot)
  useHerdrStore.getState().markAttentionSeen("default", "p1")
  render(<HerdrNotificationBridge />)
  await act(async () => { useHerdrStore.getState().applySnapshot("default", { ...snapshot, agents: snapshot.agents.map(agent => ({ ...agent, status: "done" })) }) })
  expect(useHerdrStore.getState().attentionItems("default")).toMatchObject([{ kind: "done", seen: false }])
  expect(mocks.toast).not.toHaveBeenCalled()
  expect(mocks.permission).not.toHaveBeenCalled()
})

it.each(["visible", "disabled"])("keeps the %s completion notification guard after a seen status transition", async guard => {
  useHerdrStore.getState().setEventsHealth("default", true, "sub-default")
  const emitStatus = (agentStatus: string) => useHerdrStore.getState().applySubscriptionEvent("default", {
    type: "agent_status_changed", subscriptionId: "sub-default", paneId: "p1", workspaceId: "w1", agentStatus, stateLabels: {}
  })
  emitStatus("blocked")
  useHerdrStore.getState().markAttentionSeen("default", "p1")
  if (guard === "visible") {
    vi.mocked(document.hasFocus).mockReturnValue(true)
    useHerdrNativeStore.setState({ selection: { sessionName: "default" } })
  } else useHerdrNotificationStore.setState({ done: false })
  render(<HerdrNotificationBridge />)
  await act(async () => { emitStatus("done") })
  expect(useHerdrStore.getState().attentionItems("default")).toMatchObject([{ kind: "done", seen: false }])
  expect(mocks.toast).not.toHaveBeenCalled()
  expect(mocks.permission).not.toHaveBeenCalled()
})

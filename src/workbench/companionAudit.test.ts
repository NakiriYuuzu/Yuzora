import { afterEach, beforeEach, expect, it, vi } from "vitest"
import type { CompanionEvent } from "@/lib/companionIpc"
import type { HerdrNamedSession } from "@/lib/herdrTypes"
import { useCompanionStore } from "@/state/companionStore"
import { useHerdrStore } from "@/state/herdrStore"
import { companionFixtures as fx } from "@/test/companionFixtures"

const mocks = vi.hoisted(() => ({
  invoke: vi.fn(),
  toast: Object.assign(vi.fn(), { warning: vi.fn() }),
  isPermissionGranted: vi.fn(),
  sendNotification: vi.fn(),
}))
vi.mock("@/lib/ipc", async importOriginal => ({ ...(await importOriginal<typeof import("@/lib/ipc")>()), invoke: mocks.invoke }))
vi.mock("sonner", () => ({ toast: mocks.toast }))
vi.mock("@tauri-apps/plugin-notification", () => ({ isPermissionGranted: mocks.isPermissionGranted, sendNotification: mocks.sendNotification }))
import { reportCompanionEvent } from "./companionAudit"

// Shapes as the gateway sends them (captured from the PWA end-to-end run).
const device = { ...fx.device, deviceId: "ikrCaV7JPKLNW7SjoMYrdA", label: "iPhone" }
const audit = (action: string, summary: Record<string, unknown>, target = "w1:p1", scope: "view" | "control" = "control"): CompanionEvent => ({
  type: "audit", deviceId: device.deviceId, peerId: 0, scope, action, session: "e2e", target, summary, at: "2026-10-18T03:47:38.176376458Z",
})
const controlState = (takeover: boolean, open = true): CompanionEvent => ({
  type: "control.state", deviceId: device.deviceId, peerId: 0, stream: "6", seq: open ? 1 : 2, session: "e2e", target: "w1:p1", takeover, open,
})
const logged = () => mocks.invoke.mock.calls.filter(([command]) => command === "log_event").map(([, args]) => (args as { event: Record<string, unknown> }).event)

beforeEach(() => {
  mocks.invoke.mockReset().mockResolvedValue(undefined)
  mocks.toast.mockReset()
  mocks.toast.warning.mockReset()
  mocks.isPermissionGranted.mockReset().mockResolvedValue(true)
  mocks.sendNotification.mockReset()
  useCompanionStore.setState({ devices: [device] })
  useHerdrStore.setState({
    sessions: [{ name: "e2e", running: true, default: true } as HerdrNamedSession],
    runtimesBySession: {
      e2e: { snapshot: { agents: [{ id: "a1", name: "pi", status: "idle", workspaceId: "w1", paneId: "w1:p1" }] } },
    } as never,
  })
})
afterEach(() => vi.restoreAllMocks())

it("logs a prompt as an audit without its text and shows one notice per device and pane", () => {
  reportCompanionEvent(audit("agent.prompt", { bytes: 16, length: 16, prefix: "SECRET prompt" }))
  expect(logged()).toEqual([{
    level: "info", kind: "audit", source: "companion", workspace_path: null, event: "companion_audit",
    message: "iPhone: agent.prompt on pi",
    metadata: {
      deviceId: device.deviceId, peerId: 0, scope: "control", action: "agent.prompt", session: "e2e", target: "w1:p1",
      reason: undefined, outcome: undefined, at: "2026-10-18T03:47:38.176376458Z", summary: { bytes: 16, length: 16 },
    },
  }])
  expect(mocks.toast).toHaveBeenCalledWith("iPhone sent a prompt to pi", { id: `companion-${device.deviceId}-w1:p1` })
  expect(JSON.stringify([mocks.invoke.mock.calls, mocks.toast.mock.calls])).not.toContain("SECRET")
})

it("logs a Git read by its method only, and shows no notice", () => {
  reportCompanionEvent(audit("git.read", { method: "git.diff", path: "secrets/.env", prefix: "SECRET", text: "SECRET" }, "w1", "view"))
  expect(logged()).toEqual([{
    level: "info", kind: "audit", source: "companion", workspace_path: null, event: "companion_audit",
    message: "iPhone: git.read on e2e · w1",
    metadata: {
      deviceId: device.deviceId, peerId: 0, scope: "view", action: "git.read", session: "e2e", target: "w1",
      reason: undefined, outcome: undefined, at: "2026-10-18T03:47:38.176376458Z", summary: { method: "git.diff" },
    },
  }])
  expect(mocks.toast).not.toHaveBeenCalled()
  expect(mocks.toast.warning).not.toHaveBeenCalled()
  expect(JSON.stringify(mocks.invoke.mock.calls)).not.toContain("SECRET")
  expect(JSON.stringify(mocks.invoke.mock.calls)).not.toContain(".env")
})

it("notices a control stream once, from its state, and a background takeover also notifies the system", async () => {
  reportCompanionEvent(audit("terminal.control", { resizeConfirmed: true, takeover: false }))
  expect(logged()).toHaveLength(1)
  expect(mocks.toast).not.toHaveBeenCalled()
  reportCompanionEvent(controlState(false))
  expect(mocks.toast).toHaveBeenCalledWith("iPhone is controlling pi", { id: "companion-control-6" })
  // In the foreground the notice is enough.
  const focus = vi.spyOn(document, "hasFocus").mockReturnValue(true)
  reportCompanionEvent(controlState(true))
  expect(mocks.isPermissionGranted).not.toHaveBeenCalled()

  focus.mockReturnValue(false)
  reportCompanionEvent(controlState(true))
  expect(mocks.toast.warning).toHaveBeenCalledWith("iPhone took over pi", { id: "companion-control-6" })
  await vi.waitFor(() => expect(mocks.sendNotification).toHaveBeenCalledWith({ title: "iPhone took over pi" }))

  mocks.toast.mockClear()
  mocks.toast.warning.mockClear()
  reportCompanionEvent(controlState(false, false))
  expect(mocks.toast).not.toHaveBeenCalled()
  expect(mocks.toast.warning).not.toHaveBeenCalled()
})

it("never asks for notification permission", async () => {
  mocks.isPermissionGranted.mockResolvedValue(false)
  vi.spyOn(document, "hasFocus").mockReturnValue(false)
  reportCompanionEvent(controlState(true))
  await vi.waitFor(() => expect(mocks.isPermissionGranted).toHaveBeenCalled())
  expect(mocks.sendNotification).not.toHaveBeenCalled()
})

it("names an unknown device and pane without guessing", () => {
  useCompanionStore.setState({ devices: [] })
  reportCompanionEvent(audit("pane.sendKeys", { keys: 1 }, "w1:p9"))
  expect(logged()[0].message).toBe("A phone: pane.sendKeys on e2e · w1:p9")
  expect(mocks.toast).toHaveBeenCalledWith("A phone sent keys to e2e · w1:p9", { id: `companion-${device.deviceId}-w1:p9` })
})

it("warns about lost audit records, and calls out lost release or revoke records", () => {
  reportCompanionEvent({ type: "audit.dropped", lost: 2, lostTightening: 0, rejectedOmitted: 3, since: "2026-10-18T03:47:00Z" })
  expect(mocks.toast.warning).toHaveBeenLastCalledWith("Some Companion audit records were lost (2).")
  reportCompanionEvent({ type: "audit.dropped", lost: 1, lostTightening: 1, rejectedOmitted: 0, since: "2026-10-18T03:48:00Z" })
  expect(mocks.toast.warning).toHaveBeenLastCalledWith("Companion audit records about releasing or revoking control were lost (1). Check the devices in Settings.")
  expect(logged().map(event => [event.level, event.event])).toEqual([["warn", "companion_audit_dropped"], ["warn", "companion_audit_dropped"]])
})

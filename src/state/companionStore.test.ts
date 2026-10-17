import { beforeEach, expect, it, vi } from "vitest"
import { companionFixtures as fx, companionInvoke, type CompanionReplies } from "@/test/companionFixtures"

const ipc = vi.hoisted(() => ({ invoke: vi.fn() }))
vi.mock("@/lib/ipc", async importOriginal => ({ ...(await importOriginal<typeof import("@/lib/ipc")>()), invoke: ipc.invoke }))
import { useCompanionStore } from "./companionStore"

let replies: CompanionReplies
const store = () => useCompanionStore.getState()

beforeEach(() => {
  replies = { companion_status: fx.overviewRunning, companion_devices: [fx.device] }
  ipc.invoke.mockReset().mockImplementation(companionInvoke(() => replies))
  useCompanionStore.setState({ overview: null, devices: [], claims: [], settled: {} })
})

it("upserts and removes devices from events", () => {
  store().applyEvent({ type: "device.registered", device: fx.device })
  store().applyEvent({ type: "device.updated", device: { ...fx.device, label: "Work phone" } })
  expect(store().devices).toEqual([{ ...fx.device, label: "Work phone" }])
  store().applyEvent({ type: "device.revoked", deviceId: fx.device.deviceId })
  expect(store().devices).toEqual([])
})

it("queues claims and drops one once its pairing settles", () => {
  const claim = { pairingId: "p1", deviceLabel: "iPhone", sas: "123456", via: "tailscale", tailscaleLogin: "u@example.com", userAgent: "Safari" }
  store().applyEvent({ type: "pairing.claimed", ...claim })
  store().applyEvent({ type: "pairing.claimed", ...claim, pairingId: "p2" })
  expect(store().claims.map(item => item.pairingId)).toEqual(["p1", "p2"])
  expect(store().claims[0]).toEqual(claim)
  store().applyEvent({ type: "pairing.state", pairingId: "p1", state: "rejected", reason: "cancelled" })
  expect(store().claims.map(item => item.pairingId)).toEqual(["p2"])
  expect(store().settled).toEqual({ p1: "rejected" })
})

it("drops pending claims once the gateway stops, since none can be approved", () => {
  const claim = { pairingId: "p1", deviceLabel: "iPhone", sas: "123456", via: "tailscale", tailscaleLogin: "u@example.com", userAgent: "Safari" }
  store().applyEvent({ type: "pairing.claimed", ...claim })
  store().applyEvent({ type: "status", status: fx.statusStopping })
  expect(store().claims).toHaveLength(1)
  store().applyEvent({ type: "status", status: fx.statusFailed })
  expect(store().claims).toEqual([])
})

it("re-reads the overview once a gateway stops, since the Serve record may have changed", async () => {
  useCompanionStore.setState({ overview: fx.overviewRunning })
  replies.companion_status = fx.overviewOff
  store().applyEvent({ type: "status", status: { state: "disabled" } })
  expect(store().overview?.status).toEqual({ state: "disabled" })
  await vi.waitFor(() => expect(store().overview).toEqual(fx.overviewOff))
})

it("refreshes again when an event lands while it reads", async () => {
  let resolve: () => void = () => undefined
  let first = true
  replies.companion_status = () => first ? new Promise(done => { resolve = () => done(fx.overviewOff) }) : fx.overviewRunning
  const refreshing = store().refresh()
  first = false
  store().applyEvent({ type: "device.revoked", deviceId: "other" })
  resolve()
  await refreshing
  expect(store().overview).toEqual(fx.overviewRunning)
})

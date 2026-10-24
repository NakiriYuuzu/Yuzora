import { render } from "@testing-library/react"
import { act } from "react"
import { afterEach, beforeEach, expect, it, vi } from "vitest"
import { useCompanionStore } from "@/state/companionStore"
import { companionFixtures as fx } from "@/test/companionFixtures"

const events = vi.hoisted(() => ({ listen: vi.fn(), unlisten: vi.fn() }))
vi.mock("@tauri-apps/api/event", () => ({ listen: events.listen }))
const ipc = vi.hoisted(() => ({ invoke: vi.fn() }))
vi.mock("@/lib/ipc", async importOriginal => ({ ...(await importOriginal<typeof import("@/lib/ipc")>()), invoke: ipc.invoke }))
const audit = vi.hoisted(() => ({ report: vi.fn() }))
vi.mock("./companionAudit", () => ({ reportCompanionEvent: audit.report }))
import { CompanionBridge, PEER_RECONCILE_MS } from "./CompanionBridge"

beforeEach(() => {
  events.listen.mockReset().mockResolvedValue(events.unlisten)
  events.unlisten.mockReset()
  ipc.invoke.mockReset().mockResolvedValue({ peers: [], streams: [] })
  useCompanionStore.setState({ overview: null, devices: [], peers: {}, streams: {}, streamSeq: {}, claims: [], settled: {} })
})
afterEach(() => vi.useRealTimers())

it("feeds companion events into the store and stops listening on unmount", async () => {
  const view = render(<CompanionBridge />)
  expect(events.listen).toHaveBeenCalledWith("companion://event", expect.any(Function))
  const deliver = events.listen.mock.calls[0][1] as (event: { payload: unknown }) => void
  deliver({ payload: { type: "device.registered", device: fx.device } })
  expect(useCompanionStore.getState().devices).toEqual([fx.device])
  expect(audit.report).toHaveBeenCalledWith({ type: "device.registered", device: fx.device })
  view.unmount()
  await vi.waitFor(() => expect(events.unlisten).toHaveBeenCalled())
})

const asked = () => ipc.invoke.mock.calls.filter(([name]) => name === "companion_peers").length

it("asks the gateway who is connected on an interval while it runs, and stops when it does not", async () => {
  vi.useFakeTimers({ toFake: ["setInterval", "clearInterval"] })
  useCompanionStore.setState({ overview: fx.overviewOff })
  const view = render(<CompanionBridge />)
  await act(async () => { await vi.advanceTimersByTimeAsync(PEER_RECONCILE_MS * 3) })
  expect(asked()).toBe(0)
  // A phone whose session.closed was dropped is still listed until the next ask.
  act(() => useCompanionStore.setState({ overview: fx.overviewRunning, peers: { 3: "ghost" } }))
  await act(async () => { await vi.advanceTimersByTimeAsync(PEER_RECONCILE_MS - 1) })
  expect(asked()).toBe(0)
  await act(async () => { await vi.advanceTimersByTimeAsync(1) })
  expect(asked()).toBe(1)
  expect(useCompanionStore.getState().peers).toEqual({})
  await act(async () => { await vi.advanceTimersByTimeAsync(PEER_RECONCILE_MS * 2) })
  expect(asked()).toBe(3)
  // Leaving running clears the timer.
  act(() => useCompanionStore.getState().applyEvent({ type: "status", status: fx.statusStopping }))
  await act(async () => { await vi.advanceTimersByTimeAsync(PEER_RECONCILE_MS * 3) })
  expect(asked()).toBe(3)
  act(() => useCompanionStore.getState().applyEvent({ type: "status", status: { state: "running", port: 47110 } }))
  await act(async () => { await vi.advanceTimersByTimeAsync(PEER_RECONCILE_MS) })
  expect(asked()).toBe(4)
  view.unmount()
  await act(async () => { await vi.advanceTimersByTimeAsync(PEER_RECONCILE_MS * 3) })
  expect(asked()).toBe(4)
})

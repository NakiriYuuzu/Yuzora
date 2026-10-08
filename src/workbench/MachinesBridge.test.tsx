import { act, cleanup, render } from "@testing-library/react"
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"
import { machine, snapshot, supportedCaps } from "@/test/machinesFixtures"
import { useMachinesStore } from "@/state/machinesStore"
import { MachinesBridge } from "./MachinesBridge"
import { HERDR_BINARY_SOURCE_CHANGED_EVENT, machineBackoffDelay, machinePollIntervals } from "./machinesPolicy"

const ipc = vi.hoisted(() => ({ caps: vi.fn(), list: vi.fn(), agents: vi.fn(), status: vi.fn(), windows: false }))
vi.mock("@/lib/machinesIpc", () => ({
  machinesCapabilities: ipc.caps, machinesList: ipc.list, machinesAgents: ipc.agents,
  machinesStatus: ipc.status, machinesRename: vi.fn(), machinesSetEnabled: vi.fn(), machinesRemove: vi.fn()
}))
vi.mock("@/lib/platform", async original => ({ ...await original<typeof import("@/lib/platform")>(), isWindowsPlatform: () => ipc.windows }))

const SEC = 1000
const advance = (ms: number) => act(async () => { await vi.advanceTimersByTimeAsync(ms) })
const flush = () => advance(0)
function setVisibility(state: "visible" | "hidden") {
  Object.defineProperty(document, "visibilityState", { configurable: true, get: () => state })
  document.dispatchEvent(new Event("visibilitychange"))
}
async function mount(machines = [machine("a")]) {
  ipc.caps.mockResolvedValue(supportedCaps)
  ipc.list.mockResolvedValue(machines)
  render(<MachinesBridge />)
  await flush()
}

beforeEach(() => {
  vi.useFakeTimers()
  vi.resetAllMocks()
  ipc.windows = false
  ipc.agents.mockImplementation(async (id: string) => snapshot(id))
  useMachinesStore.getState().reset()
  Object.defineProperty(document, "visibilityState", { configurable: true, get: () => "visible" })
})
afterEach(() => {
  cleanup()
  vi.useRealTimers()
})

describe("polling policy helpers", () => {
  it("uses slower intervals on Windows and caps backoff at five minutes", () => {
    expect(machinePollIntervals(false)).toEqual({ visible: 15_000, hidden: 60_000 })
    expect(machinePollIntervals(true)).toEqual({ visible: 30_000, hidden: 120_000 })
    expect(machineBackoffDelay(15_000, 1)).toBe(30_000)
    expect(machineBackoffDelay(15_000, 2)).toBe(60_000)
    expect(machineBackoffDelay(15_000, 10)).toBe(300_000)
  })
})

describe("MachinesBridge", () => {
  it("refreshes an empty catalog at the slow interval and picks up machines added outside Yuzora", async () => {
    await mount([])
    ipc.list.mockClear()
    await advance(59 * SEC)
    expect(ipc.list).not.toHaveBeenCalled()
    ipc.list.mockResolvedValue([machine("a")])
    await advance(1 * SEC)
    expect(ipc.list).toHaveBeenCalledTimes(1)
    await vi.waitFor(() => expect(ipc.agents.mock.calls.map(c => c[0])).toEqual([machine("a").id]))
  })

  it("does not touch machines when the binary is unsupported", async () => {
    ipc.caps.mockResolvedValue({ ...supportedCaps, supported: false, reason: "machines-runtime-too-old" })
    render(<MachinesBridge />)
    await advance(120 * SEC)
    expect(ipc.list).not.toHaveBeenCalled()
    expect(ipc.agents).not.toHaveBeenCalled()
    // A confirmed old version is not probed again.
    expect(ipc.caps).toHaveBeenCalledTimes(1)
  })

  it.each([
    ["an unparsable or timed-out version probe", (caps: typeof supportedCaps) => ({ ...caps, supported: false, version: null, reason: "machines-runtime-too-old" })],
    ["a missing binary", (caps: typeof supportedCaps) => ({ ...caps, supported: false, version: null, reason: "machines-binary-unavailable" })]
  ])("retries the capability probe slowly after %s and polls once it recovers", async (_name, incomplete) => {
    ipc.caps.mockResolvedValueOnce(incomplete(supportedCaps)).mockResolvedValue(supportedCaps)
    ipc.list.mockResolvedValue([machine("a")])
    render(<MachinesBridge />)
    await advance(59 * SEC)
    expect(ipc.caps).toHaveBeenCalledTimes(1)
    expect(ipc.list).not.toHaveBeenCalled()
    await advance(1 * SEC)
    expect(ipc.caps).toHaveBeenCalledTimes(2)
    expect(ipc.agents.mock.calls.map(c => c[0])).toEqual([machine("a").id])
    await advance(120 * SEC)
    expect(ipc.caps).toHaveBeenCalledTimes(2)
  })

  it("retries a failed capability call and stays idle for a remote-only source", async () => {
    ipc.caps.mockRejectedValueOnce(new Error("ipc down")).mockResolvedValue({ ...supportedCaps, supported: false, version: null, reason: "machines-local-only" })
    render(<MachinesBridge />)
    await advance(60 * SEC)
    expect(ipc.caps).toHaveBeenCalledTimes(2)
    await advance(180 * SEC)
    expect(ipc.caps).toHaveBeenCalledTimes(2)
  })

  it("polls enabled machines every 15s while visible and skips disabled ones", async () => {
    await mount([machine("a"), machine("b", { enabled: false })])
    expect(ipc.agents.mock.calls.map(c => c[0])).toEqual([machine("a").id])
    await advance(14 * SEC)
    expect(ipc.agents).toHaveBeenCalledTimes(1)
    await advance(1 * SEC)
    expect(ipc.agents).toHaveBeenCalledTimes(2)
    expect(useMachinesStore.getState().snapshotById[machine("a").id]).toBeDefined()
  })

  it("uses the Windows interval", async () => {
    ipc.windows = true
    await mount()
    await advance(29 * SEC)
    expect(ipc.agents).toHaveBeenCalledTimes(1)
    await advance(1 * SEC)
    expect(ipc.agents).toHaveBeenCalledTimes(2)
  })

  it("keeps only a 60s list refresh while hidden", async () => {
    await mount()
    setVisibility("hidden")
    ipc.list.mockClear(); ipc.agents.mockClear()
    await advance(59 * SEC)
    expect(ipc.list).not.toHaveBeenCalled()
    await advance(1 * SEC)
    expect(ipc.list).toHaveBeenCalledTimes(1)
    expect(ipc.agents).not.toHaveBeenCalled()
    setVisibility("visible")
    await flush()
    expect(ipc.agents).toHaveBeenCalledTimes(1)
  })

  it("queries at most two machines at once", async () => {
    let active = 0, peak = 0
    const release: Array<() => void> = []
    ipc.agents.mockImplementation((id: string) => new Promise(resolve => {
      active++; peak = Math.max(peak, active)
      release.push(() => { active--; resolve(snapshot(id)) })
    }))
    await mount(["a", "b", "c", "d", "e"].map(id => machine(id)))
    expect(peak).toBe(2)
    for (let index = 0; index < 5; index++) { release.shift()?.(); await flush() }
    expect(ipc.agents).toHaveBeenCalledTimes(5)
    expect(peak).toBe(2)
  })

  it("backs off exponentially after consecutive failures", async () => {
    ipc.agents.mockRejectedValue("machines-unreachable")
    await mount()
    expect(ipc.agents).toHaveBeenCalledTimes(1) // t=0
    await advance(15 * SEC) // t=15 still backing off (next at 30)
    expect(ipc.agents).toHaveBeenCalledTimes(1)
    await advance(15 * SEC) // t=30
    expect(ipc.agents).toHaveBeenCalledTimes(2)
    await advance(60 * SEC) // t=90 (next at 30+60)
    expect(ipc.agents).toHaveBeenCalledTimes(3)
  })

  it("resets the backoff after a success", async () => {
    ipc.agents.mockRejectedValueOnce("machines-unreachable")
    await mount()
    await advance(30 * SEC)
    expect(ipc.agents).toHaveBeenCalledTimes(2)
    await advance(15 * SEC)
    expect(ipc.agents).toHaveBeenCalledTimes(3)
  })

  it("never retries auth-required until the user asks for a refresh", async () => {
    ipc.agents.mockRejectedValue("machines-auth-required")
    await mount()
    await advance(10 * 60 * SEC)
    expect(ipc.agents).toHaveBeenCalledTimes(1)
    act(() => useMachinesStore.getState().requestRefresh())
    await flush()
    expect(ipc.agents).toHaveBeenCalledTimes(2)
  })

  it("keeps retrying the catalog after the startup list failed", async () => {
    ipc.caps.mockResolvedValue(supportedCaps)
    ipc.list.mockRejectedValueOnce("machines-timeout")
    render(<MachinesBridge />)
    await flush()
    expect(useMachinesStore.getState().machines).toEqual([])
    ipc.list.mockResolvedValue([machine("a")])
    await advance(machinePollIntervals(false).visible)
    expect(useMachinesStore.getState().machines).toEqual([machine("a")])
  })

  it("re-enabling a blocked machine polls it again even when no round saw it disabled", async () => {
    ipc.agents.mockRejectedValue("machines-auth-required")
    await mount()
    expect(ipc.agents).toHaveBeenCalledTimes(1)
    // Both mutations adopt their lists before the next round runs.
    act(() => useMachinesStore.setState({ machines: [machine("a", { enabled: false })] }))
    act(() => useMachinesStore.setState({ machines: [machine("a")] }))
    ipc.agents.mockImplementation(async (id: string) => snapshot(id))
    await advance(machinePollIntervals(false).visible)
    expect(ipc.agents).toHaveBeenCalledTimes(2)
  })

  it("clears auth-required block of a machine that was disabled, so re-enabling polls it again", async () => {
    ipc.agents.mockRejectedValue("machines-auth-required")
    await mount()
    expect(ipc.agents).toHaveBeenCalledTimes(1)
    ipc.list.mockResolvedValue([machine("a", { enabled: false })])
    act(() => useMachinesStore.getState().requestRefresh(false))
    await flush()
    ipc.list.mockResolvedValue([machine("a")])
    ipc.agents.mockImplementation(async (id: string) => snapshot(id))
    act(() => useMachinesStore.getState().requestRefresh(false))
    await flush()
    expect(ipc.agents).toHaveBeenCalledTimes(2)
  })

  it("a soft refresh (opening the Machines tab) keeps the auth-required block", async () => {
    ipc.agents.mockRejectedValue("machines-auth-required")
    await mount()
    act(() => useMachinesStore.getState().requestRefresh(false))
    await flush()
    expect(ipc.agents).toHaveBeenCalledTimes(1)
  })

  it("never starts a round while the previous one is unfinished", async () => {
    ipc.agents.mockImplementation(() => new Promise(() => undefined))
    await mount()
    await advance(5 * 60 * SEC)
    expect(ipc.agents).toHaveBeenCalledTimes(1)
    expect(ipc.list).toHaveBeenCalledTimes(2) // bootstrap + the single running round
  })

  it("defers a forced refresh while a round is still running", async () => {
    let finish!: () => void
    ipc.agents.mockImplementationOnce((id: string) => new Promise(resolve => { finish = () => resolve(snapshot(id)) }))
    await mount()
    act(() => useMachinesStore.getState().requestRefresh())
    await flush()
    expect(ipc.agents).toHaveBeenCalledTimes(1)
    finish()
    await flush()
    expect(ipc.agents).toHaveBeenCalledTimes(2)
  })

  it("falls back to the slow catalog refresh once the last machine disappears", async () => {
    await mount()
    ipc.list.mockResolvedValue([])
    await advance(15 * SEC)
    ipc.agents.mockClear(); ipc.list.mockClear()
    await advance(120 * SEC)
    expect(ipc.list).toHaveBeenCalledTimes(2)
    expect(ipc.agents).not.toHaveBeenCalled()
  })

  it("reloads capabilities when the binary source changes", async () => {
    await mount()
    ipc.caps.mockClear()
    window.dispatchEvent(new Event(HERDR_BINARY_SOURCE_CHANGED_EVENT))
    await flush()
    expect(ipc.caps).toHaveBeenCalledTimes(1)
  })

  it("a reachable manual status check lifts the auth block of that machine only and fetches once", async () => {
    ipc.agents.mockImplementation(async (id: string) => { throw id.startsWith("a") ? "machines-auth-required" : "machines-unreachable" })
    await mount([machine("a"), machine("b")])
    expect(ipc.agents).toHaveBeenCalledTimes(2)
    ipc.agents.mockClear()
    ipc.agents.mockImplementation(async (id: string) => snapshot(id))
    ipc.status.mockResolvedValue({ id: machine("a").id, label: "x", status: "reachable", error: null })
    await act(async () => { await useMachinesStore.getState().refreshStatus(machine("a").id) })
    await flush()
    expect(ipc.agents.mock.calls.map(c => c[0])).toEqual([machine("a").id])
    // b stays in backoff (next at 30s) while a polls normally again.
    await advance(15 * SEC)
    expect(ipc.agents.mock.calls.map(c => c[0])).toEqual([machine("a").id, machine("a").id])
  })

it("a superseded snapshot failure cannot re-block a machine a newer snapshot reached", async () => {
  await mount()
  let failOld!: () => void
  ipc.agents.mockImplementationOnce(() => new Promise((_, reject) => { failOld = () => reject("machines-auth-required") }))
  ipc.status.mockResolvedValue({ id: machine("a").id, label: "x", status: "reachable", error: null })
  // The recovery fetch stays in flight while a forced round reaches the machine.
  await act(async () => { await useMachinesStore.getState().refreshStatus(machine("a").id) })
  act(() => useMachinesStore.getState().requestRefresh(true))
  await flush()
  failOld()
  await flush()
  ipc.agents.mockClear()
  await advance(15 * SEC)
  expect(ipc.agents).toHaveBeenCalledTimes(1)
})

  it("an older in-flight failure cannot re-block a machine after a reachable status recovery", async () => {
    let fail!: () => void
    ipc.agents.mockImplementationOnce(() => new Promise((_, reject) => { fail = () => reject("machines-auth-required") }))
    await mount()
    ipc.agents.mockImplementation(async (id: string) => snapshot(id))
    ipc.status.mockResolvedValue({ id: machine("a").id, label: "x", status: "reachable", error: null })
    await act(async () => { await useMachinesStore.getState().refreshStatus(machine("a").id) })
    await flush()
    fail()
    await flush()
    ipc.agents.mockClear()
    await advance(15 * SEC)
    expect(ipc.agents).toHaveBeenCalledTimes(1)
  })

  it("does not create a poller when bootstrap finishes after unmount", async () => {
    ipc.caps.mockResolvedValue(supportedCaps)
    let release!: (machines: ReturnType<typeof machine>[]) => void
    ipc.list.mockReturnValueOnce(new Promise(resolve => { release = resolve }))
    const view = render(<MachinesBridge />)
    await flush()
    view.unmount()
    release([machine("a")])
    await flush()
    await advance(60 * SEC)
    expect(ipc.agents).not.toHaveBeenCalled()
  })

  it("does not fetch the list or poll when capabilities resolve after unmount", async () => {
    let release!: (caps: typeof supportedCaps) => void
    ipc.caps.mockReturnValueOnce(new Promise(resolve => { release = resolve }))
    ipc.list.mockResolvedValue([machine("a")])
    const view = render(<MachinesBridge />)
    await flush()
    view.unmount()
    release(supportedCaps)
    await flush()
    await advance(60 * SEC)
    expect(ipc.list).not.toHaveBeenCalled()
    expect(ipc.agents).not.toHaveBeenCalled()
  })

  it("skips a machine that was disabled while its round was already running", async () => {
    await mount([machine("a")])
    ipc.agents.mockClear()
    const release: Array<() => void> = []
    ipc.agents.mockImplementation((id: string) => new Promise(resolve => { release.push(() => resolve(snapshot(id))) }))
    ipc.list.mockResolvedValue([machine("a"), machine("b"), machine("c")])
    await advance(15 * SEC)
    // two workers busy on a and b; c is queued
    useMachinesStore.setState({ machines: [machine("a"), machine("b"), machine("c", { enabled: false })] })
    release.splice(0).forEach(r => r())
    await flush()
    expect(ipc.agents.mock.calls.map(c => c[0])).not.toContain(machine("c").id)
  })

})

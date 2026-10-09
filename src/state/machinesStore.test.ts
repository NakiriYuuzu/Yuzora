import { beforeEach, describe, expect, it, vi } from "vitest"
import { machine, snapshot, supportedCaps } from "@/test/machinesFixtures"
import { useMachinesStore } from "./machinesStore"

const ipc = vi.hoisted(() => ({
  caps: vi.fn(), list: vi.fn(), status: vi.fn(), agents: vi.fn(), rename: vi.fn(), setEnabled: vi.fn(), remove: vi.fn()
}))
vi.mock("@/lib/machinesIpc", () => ({
  machinesCapabilities: ipc.caps, machinesList: ipc.list, machinesStatus: ipc.status, machinesAgents: ipc.agents,
  machinesRename: ipc.rename, machinesSetEnabled: ipc.setEnabled, machinesRemove: ipc.remove
}))

const a = machine("a"), b = machine("b")
beforeEach(() => {
  vi.resetAllMocks()
  useMachinesStore.getState().reset()
})

describe("machinesStore", () => {
  it("ignores an older list response that resolves after a mutation result", async () => {
    let resolveOld!: (value: unknown) => void
    ipc.list.mockReturnValueOnce(new Promise((resolve) => { resolveOld = resolve }))
    const pending = useMachinesStore.getState().refreshList()
    ipc.remove.mockResolvedValue([b])
    await useMachinesStore.getState().remove(a.id)
    resolveOld([a, b])
    await pending
    expect(useMachinesStore.getState().machines).toEqual([b])
  })

  it("ignores an older list response that resolves after a newer refresh", async () => {
    let resolveOld!: (value: unknown) => void
    ipc.list.mockReturnValueOnce(new Promise((resolve) => { resolveOld = resolve }))
    const older = useMachinesStore.getState().refreshList()
    ipc.list.mockResolvedValueOnce([b])
    await useMachinesStore.getState().refreshList()
    resolveOld([a])
    await older
    expect(useMachinesStore.getState().machines).toEqual([b])
  })

  it("loads capabilities and the list", async () => {
    ipc.caps.mockResolvedValue(supportedCaps)
    ipc.list.mockResolvedValue([a, b])
    await useMachinesStore.getState().loadCapabilities()
    await useMachinesStore.getState().refreshList()
    expect(useMachinesStore.getState().capabilities).toEqual(supportedCaps)
    expect(useMachinesStore.getState().machines).toEqual([a, b])
  })

  it("clears machine-derived state when capabilities report no support", async () => {
    useMachinesStore.setState({ machines: [a], snapshotById: { [a.id]: snapshot(a.id) }, errorById: { [a.id]: "x" } })
    ipc.caps.mockResolvedValue({ ...supportedCaps, supported: false })
    await useMachinesStore.getState().loadCapabilities()
    const state = useMachinesStore.getState()
    expect(state.machines).toEqual([])
    expect(state.snapshotById).toEqual({})
    expect(state.errorById).toEqual({})
  })

it("an in-flight list cannot restore machines after support is lost", async () => {
  let resolveOld: (machines: typeof a[]) => void = () => {}
  ipc.list.mockReturnValueOnce(new Promise((resolve) => { resolveOld = resolve }))
  const pending = useMachinesStore.getState().refreshList()
  ipc.caps.mockResolvedValue({ ...supportedCaps, supported: false })
  await useMachinesStore.getState().loadCapabilities()
  resolveOld([a])
  await pending
  expect(useMachinesStore.getState().machines).toEqual([])
})

  it("a successful snapshot supersedes an older manual status result", async () => {
    useMachinesStore.setState({ machines: [a], statusById: { [a.id]: { id: a.id, status: "auth-required" } as never } })
    ipc.agents.mockResolvedValueOnce(snapshot(a.id))
    await useMachinesStore.getState().refreshSnapshot(a.id)
    expect(useMachinesStore.getState().statusById[a.id]).toBeUndefined()
  })

  it("keeps the previous snapshot and marks it stale when a refresh fails", async () => {
    useMachinesStore.setState({ machines: [a] })
    ipc.agents.mockResolvedValueOnce(snapshot(a.id))
    await useMachinesStore.getState().refreshSnapshot(a.id)
    ipc.agents.mockRejectedValueOnce("machines-unreachable: down")
    const result = await useMachinesStore.getState().refreshSnapshot(a.id)
    const state = useMachinesStore.getState()
    expect(result).toEqual({ ok: false, code: "machines-unreachable" })
    expect(state.snapshotById[a.id]).toEqual(snapshot(a.id))
    expect(state.staleById[a.id]).toBe(true)
    expect(state.errorById[a.id]).toBe("machines-unreachable: down")
    ipc.agents.mockResolvedValueOnce(snapshot(a.id))
    await useMachinesStore.getState().refreshSnapshot(a.id)
    expect(useMachinesStore.getState().staleById[a.id]).toBeUndefined()
    expect(useMachinesStore.getState().errorById[a.id]).toBeUndefined()
  })

  it("ignores machines-busy without recording an error", async () => {
    useMachinesStore.setState({ machines: [a] })
    ipc.agents.mockRejectedValue("machines-busy")
    expect(await useMachinesStore.getState().refreshSnapshot(a.id)).toEqual({ ok: false, code: "machines-busy" })
    expect(useMachinesStore.getState().errorById).toEqual({})
  })

  it("overwrites the list with the mutation result and prunes removed machines", async () => {
    useMachinesStore.setState({ machines: [a, b], snapshotById: { [a.id]: snapshot(a.id), [b.id]: snapshot(b.id) }, errorById: { [a.id]: "x" } })
    ipc.remove.mockResolvedValue([b])
    await useMachinesStore.getState().remove(a.id)
    const state = useMachinesStore.getState()
    expect(state.machines).toEqual([b])
    expect(Object.keys(state.snapshotById)).toEqual([b.id])
    expect(state.errorById).toEqual({})
    const renamed = { ...b, label: "New" }
    ipc.rename.mockResolvedValue([renamed])
    await useMachinesStore.getState().rename(b.id, "New")
    expect(useMachinesStore.getState().machines).toEqual([renamed])
    ipc.setEnabled.mockResolvedValue([{ ...renamed, enabled: false }])
    await useMachinesStore.getState().setEnabled(b.id, false)
    expect(useMachinesStore.getState().machines[0].enabled).toBe(false)
  })

  it("records list failures without dropping the previous list", async () => {
    useMachinesStore.setState({ machines: [a] })
    ipc.list.mockRejectedValue("machines-binary-unavailable")
    await useMachinesStore.getState().refreshList()
    expect(useMachinesStore.getState().machines).toEqual([a])
    expect(useMachinesStore.getState().listError).toBe("machines-binary-unavailable")
  })

  it("clears a stale error when a status check succeeds, and ignores machines-busy", async () => {
    useMachinesStore.setState({ machines: [a], errorById: { [a.id]: "machines-unreachable: down" } })
    ipc.status.mockResolvedValueOnce({ id: a.id, label: a.label, status: "reachable", error: null })
    await useMachinesStore.getState().refreshStatus(a.id)
    expect(useMachinesStore.getState().errorById[a.id]).toBeUndefined()
    expect(useMachinesStore.getState().statusById[a.id].status).toBe("reachable")
    ipc.status.mockRejectedValueOnce("machines-busy")
    expect(await useMachinesStore.getState().refreshStatus(a.id)).toBeNull()
    expect(useMachinesStore.getState().errorById[a.id]).toBeUndefined()
  })

  it("requestRefresh records whether the round is forced", () => {
    useMachinesStore.getState().requestRefresh(false)
    expect(useMachinesStore.getState().refreshForce).toBe(false)
    useMachinesStore.getState().requestRefresh()
    expect(useMachinesStore.getState().refreshForce).toBe(true)
  })

})

const deferred = <T,>() => {
  let resolve!: (value: T) => void, reject!: (reason: unknown) => void
  const promise = new Promise<T>((res, rej) => { resolve = res; reject = rej })
  return { promise, resolve, reject }
}
const unsupportedCaps = { ...supportedCaps, supported: false, reason: "machines-runtime-too-old" }

describe("machinesStore concurrency", () => {
  it("a snapshot rejected as busy does not supersede the one still in flight", async () => {
    useMachinesStore.setState({ machines: [a] })
    let resolveFirst!: (value: ReturnType<typeof snapshot>) => void
    ipc.agents.mockReturnValueOnce(new Promise((resolve) => { resolveFirst = resolve }))
    const first = useMachinesStore.getState().refreshSnapshot(a.id)
    ipc.agents.mockRejectedValueOnce("machines-busy")
    await expect(useMachinesStore.getState().refreshSnapshot(a.id)).resolves.toEqual({ ok: false, code: "machines-busy" })
    resolveFirst(snapshot(a.id))
    await expect(first).resolves.toEqual({ ok: true, code: null })
    expect(useMachinesStore.getState().snapshotById[a.id]).toEqual(snapshot(a.id))
  })

  it("a superseded list refresh resolves to the newest list instead of the stale cache", async () => {
    let resolveOld!: (machines: typeof a[]) => void
    let resolveNew!: (machines: typeof a[]) => void
    ipc.list.mockReturnValueOnce(new Promise((resolve) => { resolveOld = resolve }))
    ipc.list.mockReturnValueOnce(new Promise((resolve) => { resolveNew = resolve }))
    const older = useMachinesStore.getState().refreshList()
    const newer = useMachinesStore.getState().refreshList()
    resolveOld([a])
    await Promise.resolve()
    resolveNew([a, b])
    await expect(older).resolves.toEqual([a, b])
    await expect(newer).resolves.toEqual([a, b])
  })

  it("drops a snapshot or status that resolves after its machine was disabled", async () => {
    useMachinesStore.setState({ machines: [a] })
    let resolveSnapshot!: (value: ReturnType<typeof snapshot>) => void
    let resolveStatus!: (value: unknown) => void
    ipc.agents.mockReturnValueOnce(new Promise((resolve) => { resolveSnapshot = resolve }))
    ipc.status.mockReturnValueOnce(new Promise((resolve) => { resolveStatus = resolve }))
    const pendingSnapshot = useMachinesStore.getState().refreshSnapshot(a.id)
    const pendingStatus = useMachinesStore.getState().refreshStatus(a.id)
    ipc.setEnabled.mockResolvedValueOnce([machine("a", { enabled: false })])
    await useMachinesStore.getState().setEnabled(a.id, false)
    resolveSnapshot(snapshot(a.id))
    resolveStatus({ id: a.id, label: "x", status: "reachable", error: null })
    await pendingSnapshot
    await pendingStatus
    expect(useMachinesStore.getState().snapshotById[a.id]).toBeUndefined()
    expect(useMachinesStore.getState().statusById[a.id]).toBeUndefined()
  })

  it("a failing manual status verdict marks the cached agents stale", async () => {
    useMachinesStore.setState({ machines: [a], snapshotById: { [a.id]: snapshot(a.id) } })
    ipc.status.mockResolvedValueOnce({ id: a.id, label: "x", status: "auth-required", error: "denied" })
    await useMachinesStore.getState().refreshStatus(a.id)
    expect(useMachinesStore.getState().staleById[a.id]).toBe(true)
  })

  it("treats a mutation whose relist failed as committed and relists", async () => {
    useMachinesStore.setState({ machines: [a] })
    ipc.rename.mockRejectedValueOnce("machines-relist-failed: timeout")
    ipc.list.mockResolvedValueOnce([machine("a", { label: "renamed" })])
    await expect(useMachinesStore.getState().rename(a.id, "renamed")).resolves.toBeUndefined()
    await vi.waitFor(() => expect(useMachinesStore.getState().machines).toEqual([machine("a", { label: "renamed" })]))
  })

it("drops a disabled machine's snapshot and status so re-enabling cannot show old agents", async () => {
  useMachinesStore.setState({ machines: [a], snapshotById: { [a.id]: snapshot(a.id) }, statusById: { [a.id]: { id: a.id, status: "reachable" } as never } })
  ipc.setEnabled.mockResolvedValueOnce([machine("a", { enabled: false })])
  await useMachinesStore.getState().setEnabled(a.id, false)
  ipc.setEnabled.mockResolvedValueOnce([a])
  await useMachinesStore.getState().setEnabled(a.id, true)
  const state = useMachinesStore.getState()
  expect(state.snapshotById[a.id]).toBeUndefined()
  expect(state.statusById[a.id]).toBeUndefined()
})

  it("a superseded capability load resolves to the latest result instead of null", async () => {
    let resolveOld!: (caps: typeof supportedCaps) => void
    let resolveNew!: (caps: typeof supportedCaps) => void
    ipc.caps.mockReturnValueOnce(new Promise((resolve) => { resolveOld = resolve }))
    ipc.caps.mockReturnValueOnce(new Promise((resolve) => { resolveNew = resolve }))
    const older = useMachinesStore.getState().loadCapabilities()
    const newer = useMachinesStore.getState().loadCapabilities()
    resolveOld(supportedCaps)
    await Promise.resolve()
    resolveNew(supportedCaps)
    await expect(older).resolves.toEqual(supportedCaps)
    await expect(newer).resolves.toEqual(supportedCaps)
  })

  it("a mutation stuck in an old epoch does not block mutations after support returns", async () => {
    useMachinesStore.setState({ machines: [a] })
    ipc.rename.mockReturnValueOnce(new Promise(() => {}))
    void useMachinesStore.getState().rename(a.id, "stuck")
    ipc.caps.mockResolvedValueOnce({ ...supportedCaps, supported: false })
    await useMachinesStore.getState().loadCapabilities()
    ipc.caps.mockResolvedValueOnce(supportedCaps)
    await useMachinesStore.getState().loadCapabilities()
    ipc.setEnabled.mockResolvedValueOnce([machine("a", { enabled: false })])
    await useMachinesStore.getState().setEnabled(a.id, false)
    expect(ipc.setEnabled).toHaveBeenCalledTimes(1)
    expect(useMachinesStore.getState().machines).toEqual([machine("a", { enabled: false })])
  })

  it("marks a snapshot superseded by a newer one as stale, success or failure", async () => {
    useMachinesStore.setState({ machines: [a] })
    let failOld!: () => void
    ipc.agents.mockReturnValueOnce(new Promise((_, reject) => { failOld = () => reject("machines-auth-required") }))
    const older = useMachinesStore.getState().refreshSnapshot(a.id)
    ipc.agents.mockResolvedValueOnce(snapshot(a.id))
    await expect(useMachinesStore.getState().refreshSnapshot(a.id)).resolves.toEqual({ ok: true, code: null })
    failOld()
    await expect(older).resolves.toEqual({ ok: false, code: "machines-auth-required", stale: true })
  })

  it("rule 1: an older unsupported capability probe cannot wipe a newer supported result", async () => {
    const older = deferred<typeof supportedCaps>()
    ipc.caps.mockReturnValueOnce(older.promise).mockResolvedValueOnce(supportedCaps)
    const first = useMachinesStore.getState().loadCapabilities()
    await useMachinesStore.getState().loadCapabilities()
    useMachinesStore.setState({ machines: [a] })
    older.resolve(unsupportedCaps)
    await first
    expect(useMachinesStore.getState().capabilities).toEqual(supportedCaps)
    expect(useMachinesStore.getState().machines).toEqual([a])
  })

  it("rule 1: an older failed capability probe cannot overwrite a newer success", async () => {
    const older = deferred<never>()
    ipc.caps.mockReturnValueOnce(older.promise).mockResolvedValueOnce(supportedCaps)
    const first = useMachinesStore.getState().loadCapabilities()
    await useMachinesStore.getState().loadCapabilities()
    useMachinesStore.setState({ machines: [a] })
    older.reject("boom")
    await first
    const state = useMachinesStore.getState()
    expect(state.capabilities).toEqual(supportedCaps)
    expect(state.capabilitiesError).toBeNull()
    expect(state.machines).toEqual([a])
  })

  it("rule 2: an older list failure cannot show an error over a newer list", async () => {
    const older = deferred<never>()
    ipc.list.mockReturnValueOnce(older.promise).mockResolvedValueOnce([b])
    const first = useMachinesStore.getState().refreshList()
    await useMachinesStore.getState().refreshList()
    older.reject("machines-binary-unavailable")
    await first
    expect(useMachinesStore.getState().listError).toBeNull()
    expect(useMachinesStore.getState().machines).toEqual([b])
  })

  it("rule 2: loading stays on until the newest list request finishes", async () => {
    const older = deferred<typeof a[]>(), newer = deferred<typeof a[]>()
    ipc.list.mockReturnValueOnce(older.promise).mockReturnValueOnce(newer.promise)
    const first = useMachinesStore.getState().refreshList()
    const second = useMachinesStore.getState().refreshList()
    older.resolve([a])
    // The superseded call now waits for the newest request, so only flush its own settlement here.
    for (let i = 0; i < 5; i++) await Promise.resolve()
    expect(useMachinesStore.getState().loading).toBe(true)
    newer.resolve([b])
    await expect(second).resolves.toEqual([b])
    await expect(first).resolves.toEqual([b])
    expect(useMachinesStore.getState().loading).toBe(false)
  })

  it("rule 2: loading is cleared even when a mutation result supersedes the newest list", async () => {
    const pending = deferred<typeof a[]>()
    ipc.list.mockReturnValueOnce(pending.promise)
    const list = useMachinesStore.getState().refreshList()
    ipc.remove.mockResolvedValue([b])
    await useMachinesStore.getState().remove(a.id)
    pending.resolve([a])
    await list
    expect(useMachinesStore.getState().loading).toBe(false)
    expect(useMachinesStore.getState().machines).toEqual([b])
  })

  it("rule 2: losing support clears a pending loading flag", async () => {
    ipc.list.mockReturnValueOnce(new Promise(() => undefined))
    void useMachinesStore.getState().refreshList()
    ipc.caps.mockResolvedValue(unsupportedCaps)
    await useMachinesStore.getState().loadCapabilities()
    expect(useMachinesStore.getState().loading).toBe(false)
  })

  it("rule 3: mutations are serialized and adopt in call order", async () => {
    const first = deferred<typeof a[]>()
    ipc.rename.mockReturnValueOnce(first.promise)
    ipc.setEnabled.mockResolvedValueOnce([{ ...a, label: "R", enabled: false }])
    const rename = useMachinesStore.getState().rename(a.id, "R")
    const toggle = useMachinesStore.getState().setEnabled(a.id, false)
    await Promise.resolve()
    expect(ipc.setEnabled).not.toHaveBeenCalled()
    first.resolve([{ ...a, label: "R" }])
    await Promise.all([rename, toggle])
    expect(ipc.setEnabled).toHaveBeenCalledTimes(1)
    expect(useMachinesStore.getState().machines).toEqual([{ ...a, label: "R", enabled: false }])
  })

  it("rule 3: a failed mutation rejects its caller but does not block the queue", async () => {
    ipc.rename.mockRejectedValueOnce("machines-invalid-label")
    ipc.remove.mockResolvedValueOnce([b])
    const failing = useMachinesStore.getState().rename(a.id, "")
    const next = useMachinesStore.getState().remove(a.id)
    await expect(failing).rejects.toBe("machines-invalid-label")
    await next
    expect(useMachinesStore.getState().machines).toEqual([b])
  })

  it("rule 4: a mutation resolving after reset does not repopulate the store", async () => {
    const pending = deferred<typeof a[]>()
    ipc.remove.mockReturnValueOnce(pending.promise)
    const removing = useMachinesStore.getState().remove(a.id)
    await Promise.resolve()
    expect(ipc.remove).toHaveBeenCalled()
    useMachinesStore.getState().reset()
    pending.resolve([b])
    await removing
    expect(useMachinesStore.getState().machines).toEqual([])
  })

  it("rule 4: a queued mutation started before reset never runs", async () => {
    const first = deferred<typeof a[]>()
    ipc.rename.mockReturnValueOnce(first.promise)
    const one = useMachinesStore.getState().rename(a.id, "x")
    const two = useMachinesStore.getState().setEnabled(a.id, false)
    useMachinesStore.getState().reset()
    first.resolve([a])
    await Promise.all([one, two])
    expect(ipc.setEnabled).not.toHaveBeenCalled()
    expect(useMachinesStore.getState().machines).toEqual([])
  })

  it("rule 4: snapshot and status results arriving after reset are dropped", async () => {
    useMachinesStore.setState({ machines: [a] })
    const snap = deferred<ReturnType<typeof snapshot>>(), stat = deferred<never>()
    ipc.agents.mockReturnValueOnce(snap.promise)
    ipc.status.mockReturnValueOnce(stat.promise)
    const p1 = useMachinesStore.getState().refreshSnapshot(a.id)
    const p2 = useMachinesStore.getState().refreshStatus(a.id)
    useMachinesStore.getState().reset()
    useMachinesStore.setState({ machines: [a] })
    snap.resolve(snapshot(a.id))
    stat.reject("machines-unreachable: x")
    await Promise.all([p1, p2])
    const state = useMachinesStore.getState()
    expect(state.snapshotById).toEqual({})
    expect(state.errorById).toEqual({})
  })

  it("rule 5: a snapshot failure for a removed machine records no error", async () => {
    useMachinesStore.setState({ machines: [a] })
    const pending = deferred<never>()
    ipc.agents.mockReturnValueOnce(pending.promise)
    const call = useMachinesStore.getState().refreshSnapshot(a.id)
    ipc.remove.mockResolvedValue([])
    await useMachinesStore.getState().remove(a.id)
    pending.reject("machines-unknown-machine")
    await call
    expect(useMachinesStore.getState().errorById).toEqual({})
  })

  it("rule 5: an older snapshot cannot overwrite a newer one or add a stale error", async () => {
    useMachinesStore.setState({ machines: [a] })
    const older = deferred<never>()
    ipc.agents.mockReturnValueOnce(older.promise).mockResolvedValueOnce(snapshot(a.id))
    const first = useMachinesStore.getState().refreshSnapshot(a.id)
    await useMachinesStore.getState().refreshSnapshot(a.id)
    older.reject("machines-unreachable: late")
    await first
    expect(useMachinesStore.getState().errorById).toEqual({})
    expect(useMachinesStore.getState().snapshotById[a.id]).toBeDefined()
  })

  it("rule 5: an older status failure cannot override a newer success; removed machines are ignored", async () => {
    useMachinesStore.setState({ machines: [a] })
    const older = deferred<never>()
    ipc.status.mockReturnValueOnce(older.promise).mockResolvedValueOnce({ id: a.id, label: a.label, status: "reachable", error: null })
    const first = useMachinesStore.getState().refreshStatus(a.id)
    await useMachinesStore.getState().refreshStatus(a.id)
    older.reject("machines-auth-required")
    await first
    expect(useMachinesStore.getState().errorById).toEqual({})
    expect(useMachinesStore.getState().statusById[a.id].status).toBe("reachable")
  })

  it("rule 5: an older snapshot failure cannot restore an error over a newer reachable status", async () => {
    useMachinesStore.setState({ machines: [a] })
    const older = deferred<never>()
    ipc.agents.mockReturnValueOnce(older.promise)
    ipc.status.mockResolvedValueOnce({ id: a.id, label: a.label, status: "reachable", error: null })
    const poll = useMachinesStore.getState().refreshSnapshot(a.id)
    await useMachinesStore.getState().refreshStatus(a.id)
    older.reject("machines-unreachable: late")
    await expect(poll).resolves.toMatchObject({ stale: true })
    expect(useMachinesStore.getState().errorById).toEqual({})
    expect(useMachinesStore.getState().statusById[a.id].status).toBe("reachable")
  })

  it("rule 5: an older snapshot success cannot clear a newer auth-required verdict", async () => {
    useMachinesStore.setState({ machines: [a] })
    const older = deferred<ReturnType<typeof snapshot>>()
    ipc.agents.mockReturnValueOnce(older.promise)
    ipc.status.mockResolvedValueOnce({ id: a.id, label: a.label, status: "auth-required", error: null })
    const poll = useMachinesStore.getState().refreshSnapshot(a.id)
    await useMachinesStore.getState().refreshStatus(a.id)
    older.resolve(snapshot(a.id))
    await expect(poll).resolves.toMatchObject({ stale: true })
    expect(useMachinesStore.getState().statusById[a.id].status).toBe("auth-required")
  })

  it("rule 5: a status check rejected as busy does not supersede the snapshot still in flight", async () => {
    useMachinesStore.setState({ machines: [a] })
    const inFlight = deferred<ReturnType<typeof snapshot>>()
    ipc.agents.mockReturnValueOnce(inFlight.promise)
    ipc.status.mockRejectedValueOnce("machines-busy")
    const poll = useMachinesStore.getState().refreshSnapshot(a.id)
    await expect(useMachinesStore.getState().refreshStatus(a.id)).resolves.toBeNull()
    inFlight.resolve(snapshot(a.id))
    await expect(poll).resolves.toEqual({ ok: true, code: null })
    expect(useMachinesStore.getState().snapshotById[a.id]).toEqual(snapshot(a.id))
  })

  it("rule 6: only a reachable status check emits a recover signal for that machine", async () => {
    useMachinesStore.setState({ machines: [a, b] })
    ipc.status.mockResolvedValueOnce({ id: a.id, label: a.label, status: "auth-required", error: null })
    await useMachinesStore.getState().refreshStatus(a.id)
    expect(useMachinesStore.getState().recoverSignal).toBeNull()
    ipc.status.mockResolvedValueOnce({ id: b.id, label: b.label, status: "reachable", error: null })
    await useMachinesStore.getState().refreshStatus(b.id)
    const first = useMachinesStore.getState().recoverSignal
    expect(first).toMatchObject({ id: b.id })
    ipc.status.mockResolvedValueOnce({ id: b.id, label: b.label, status: "reachable", error: null })
    await useMachinesStore.getState().refreshStatus(b.id)
    expect(useMachinesStore.getState().recoverSignal).not.toBe(first)
  })
})

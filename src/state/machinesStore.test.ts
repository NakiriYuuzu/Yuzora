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

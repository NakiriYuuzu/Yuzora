import { create } from "zustand"
import {
  machinesAgents,
  machinesCapabilities,
  machinesList,
  machinesRemove,
  machinesRename,
  machinesSetEnabled,
  machinesStatus
} from "@/lib/machinesIpc"
import { parseMachineError } from "@/lib/machinesErrors"
import type { HerdrMachine, HerdrMachineSnapshot, HerdrMachineStatus, HerdrMachinesCapabilities } from "@/lib/machinesTypes"

export interface MachineRefreshResult {
  ok: boolean
  /** Machines error code when the call failed (`null` on success or an unclassified failure). */
  code: string | null
}

interface MachinesState {
  capabilities: HerdrMachinesCapabilities | null
  capabilitiesError: string | null
  machines: HerdrMachine[]
  listError: string | null
  statusById: Record<string, HerdrMachineStatus>
  snapshotById: Record<string, HerdrMachineSnapshot>
  /** Snapshot kept from an earlier success while the latest refresh failed. */
  staleById: Record<string, boolean>
  errorById: Record<string, string>
  loading: boolean
  /** Bumped by `requestRefresh`; the Bridge forces a round (and clears auth/backoff blocks). */
  refreshNonce: number
  /** Whether the latest `requestRefresh` also clears auth/backoff blocks. */
  refreshForce: boolean
  loadCapabilities: () => Promise<HerdrMachinesCapabilities | null>
  refreshList: () => Promise<HerdrMachine[] | null>
  refreshStatus: (id: string) => Promise<HerdrMachineStatus | null>
  refreshSnapshot: (id: string) => Promise<MachineRefreshResult>
  rename: (id: string, label: string) => Promise<void>
  setEnabled: (id: string, enabled: boolean) => Promise<void>
  remove: (id: string) => Promise<void>
  requestRefresh: (force?: boolean) => void
  reset: () => void
}

const messageOf = (cause: unknown) => (cause instanceof Error ? cause.message : String(cause))

function prune<T>(record: Record<string, T>, machines: HerdrMachine[]): Record<string, T> {
  const ids = new Set(machines.map((machine) => machine.id))
  return Object.fromEntries(Object.entries(record).filter(([id]) => ids.has(id)))
}

const initial = {
  capabilities: null,
  capabilitiesError: null,
  machines: [],
  listError: null,
  statusById: {},
  snapshotById: {},
  staleById: {},
  errorById: {},
  loading: false,
  refreshNonce: 0,
  refreshForce: true
} satisfies Partial<MachinesState>

export const useMachinesStore = create<MachinesState>((set, get) => {
  /** Advances whenever a list is adopted; an older in-flight `machinesList()` must not overwrite a newer one. */
  let listGeneration = 0
  /** The list returned by a mutation (or refresh) is authoritative: drop state of vanished machines. */
  const adopt = (machines: HerdrMachine[]) => {
    listGeneration += 1
    set((state) => ({
    machines,
    listError: null,
    statusById: prune(state.statusById, machines),
    snapshotById: prune(state.snapshotById, machines),
    staleById: prune(state.staleById, machines),
    errorById: prune(state.errorById, machines)
    }))
  }
  return {
    ...initial,
    async loadCapabilities() {
      try {
        const capabilities = await machinesCapabilities()
        set({ capabilities, capabilitiesError: null })
        return capabilities
      } catch (cause) {
        set({ capabilities: null, capabilitiesError: messageOf(cause) })
        return null
      }
    },
    async refreshList() {
      set({ loading: true })
      const generation = ++listGeneration
      try {
        const machines = await machinesList()
        // A newer refresh or a mutation result was adopted meanwhile: this response is stale.
        if (generation !== listGeneration) return get().machines
        adopt(machines)
        return machines
      } catch (cause) {
        set({ listError: messageOf(cause) })
        return null
      } finally {
        set({ loading: false })
      }
    },
    async refreshStatus(id) {
      try {
        const status = await machinesStatus(id)
        set((state) => {
          const { [id]: _error, ...errorById } = state.errorById
          return { statusById: { ...state.statusById, [id]: status }, errorById }
        })
        return status
      } catch (cause) {
        // A busy manager is not a machine failure; stay silent.
        if (messageOf(cause).startsWith("machines-busy")) return null
        set((state) => ({ errorById: { ...state.errorById, [id]: messageOf(cause) } }))
        return null
      }
    },
    async refreshSnapshot(id) {
      try {
        const snapshot = await machinesAgents(id)
        // The machine may have been removed while the call was in flight.
        if (!get().machines.some((machine) => machine.id === id)) return { ok: true, code: null }
        set((state) => {
          const { [id]: _error, ...errorById } = state.errorById
          const { [id]: _stale, ...staleById } = state.staleById
          return { snapshotById: { ...state.snapshotById, [id]: snapshot }, errorById, staleById }
        })
        return { ok: true, code: null }
      } catch (cause) {
        const raw = messageOf(cause)
        // A busy manager is not a machine failure; keep everything as is.
        if (raw.startsWith("machines-busy")) return { ok: false, code: "machines-busy" }
        set((state) => ({
          errorById: { ...state.errorById, [id]: raw },
          staleById: id in state.snapshotById ? { ...state.staleById, [id]: true } : state.staleById
        }))
        return { ok: false, code: parseMachineError(raw).code }
      }
    },
    async rename(id, label) { adopt(await machinesRename(id, label)) },
    async setEnabled(id, enabled) { adopt(await machinesSetEnabled(id, enabled)) },
    async remove(id) { adopt(await machinesRemove(id)) },
    requestRefresh(force = true) { set((state) => ({ refreshNonce: state.refreshNonce + 1, refreshForce: force })) },
    reset() { listGeneration += 1; set({ ...initial }) }
  }
})

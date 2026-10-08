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
  /** A newer snapshot of the same machine superseded this call; its outcome must not be acted on. */
  stale?: boolean
}

interface MachinesState {
  capabilities: HerdrMachinesCapabilities | null
  capabilitiesError: string | null
  machines: HerdrMachine[]
  listError: string | null
  /** True once a machine list loaded successfully in this epoch (an empty list included). */
  listLoaded: boolean
  statusById: Record<string, HerdrMachineStatus>
  snapshotById: Record<string, HerdrMachineSnapshot>
  /** Snapshot kept from an earlier success while the latest refresh failed. */
  staleById: Record<string, boolean>
  errorById: Record<string, string>
  loading: boolean
  /** Bumped when a manual status check finds a machine reachable; the Bridge lifts that machine's poll block. */
  recoverSignal: { id: string; seq: number } | null
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

/** Keep per-machine data only for enabled machines: a disabled one must not show old agents when re-enabled. */
function prune<T>(record: Record<string, T>, machines: HerdrMachine[]): Record<string, T> {
  const ids = new Set(machines.filter((machine) => machine.enabled).map((machine) => machine.id))
  return Object.fromEntries(Object.entries(record).filter(([id]) => ids.has(id)))
}

/*
 * Concurrency rules (each async action only lets its own result land when nothing newer superseded it):
 *  1. loadCapabilities: latest call wins (capsSeq), on success AND failure; a stale call returns the current state.
 *  2. refreshList: a result/error lands only if no newer list was started or adopted (listGeneration); `loading`
 *     is cleared only by the newest refreshList call (listRequest), never by an older one.
 *  3. Mutations (rename/setEnabled/remove) run one at a time through a promise queue, so list adoption follows
 *     call order; a failed mutation does not block the queue.
 *  4. Losing support, a failed capability probe or reset() advances `epoch`: every in-flight call from the old
 *     epoch (list, mutation, status, snapshot) drops its result.
 *  5. refreshStatus / refreshSnapshot: the latest call per machine and kind wins; results for machines that no
 *     longer exist are dropped (errors included).
 *  6. A reachable status check emits `recoverSignal` for that machine only; the Bridge lifts just its poll block.
 */

const initial = {
  capabilities: null,
  capabilitiesError: null,
  machines: [],
  listError: null,
  listLoaded: false,
  statusById: {},
  snapshotById: {},
  staleById: {},
  errorById: {},
  loading: false,
  recoverSignal: null,
  refreshNonce: 0,
  refreshForce: true
} satisfies Partial<MachinesState>

/** Machine-derived state that must not outlive machines support. */
const derivedInitial = {
  machines: [],
  listError: null,
  listLoaded: false,
  statusById: {},
  snapshotById: {},
  staleById: {},
  errorById: {},
  loading: false
} satisfies Partial<MachinesState>

export const useMachinesStore = create<MachinesState>((set, get) => {
  let epoch = 0
  let capsSeq = 0
  let capsLatest: Promise<HerdrMachinesCapabilities | null> = Promise.resolve(null)
  /** Advances whenever a list is requested or adopted; an older in-flight `machinesList()` must not overwrite a newer one. */
  let listGeneration = 0
  /** The newest refreshList call; only it may clear `loading`. */
  let listRequest = 0
  let queue: Promise<unknown> = Promise.resolve()
  const statusSeq = new Map<string, number>()
  const snapshotSeq = new Map<string, number>()
  let recoverSeq = 0
  // Per-machine results only land for machines that are still listed AND enabled: a result started before a
  // disable must not repopulate the state the disable just dropped.
  const exists = (id: string) => get().machines.some((machine) => machine.id === id && machine.enabled)
  // A new epoch also starts a fresh mutation queue, so a call stuck in the old epoch cannot block it.
  const invalidate = () => { epoch += 1; listGeneration += 1; listRequest += 1; queue = Promise.resolve() }
  /** The list returned by a mutation (or refresh) is authoritative: drop state of vanished machines. */
  const adopt = (machines: HerdrMachine[]) => {
    listGeneration += 1
    set((state) => ({
      machines,
      listError: null,
      listLoaded: true,
      statusById: prune(state.statusById, machines),
      snapshotById: prune(state.snapshotById, machines),
      staleById: prune(state.staleById, machines),
      errorById: prune(state.errorById, machines)
    }))
  }
  const mutate = (call: () => Promise<HerdrMachine[]>): Promise<void> => {
    const mine = epoch
    const run = queue.then(async () => {
      if (mine !== epoch) return
      let machines: HerdrMachine[]
      try {
        machines = await call()
      } catch (cause) {
        // The catalog change was committed; only the follow-up list failed. Report success and relist.
        if (!messageOf(cause).startsWith("machines-relist-failed")) throw cause
        if (mine === epoch) void get().refreshList()
        return
      }
      if (mine === epoch) adopt(machines)
    })
    queue = run.catch(() => undefined)
    return run
  }
  return {
    ...initial,
    loadCapabilities() {
      const seq = ++capsSeq
      // A superseded call resolves to the latest call's result, never to a not-yet-loaded null.
      const latest = () => (capsLatest === run ? Promise.resolve(get().capabilities) : capsLatest)
      const run: Promise<HerdrMachinesCapabilities | null> = (async () => {
        try {
          const capabilities = await machinesCapabilities()
          if (seq !== capsSeq) return latest()
          if (capabilities.supported) {
            set({ capabilities, capabilitiesError: null })
          } else {
            // Losing support invalidates everything derived from the machine list, including in-flight calls.
            invalidate()
            set({ ...derivedInitial, capabilities, capabilitiesError: null })
          }
          return capabilities
        } catch (cause) {
          if (seq !== capsSeq) return latest()
          invalidate()
          set({ ...derivedInitial, capabilities: null, capabilitiesError: messageOf(cause) })
          return null
        }
      })()
      capsLatest = run
      return run
    },
    async refreshList() {
      const mine = epoch
      const request = ++listRequest
      const generation = ++listGeneration
      set({ loading: true })
      try {
        const machines = await machinesList()
        // A newer refresh, a mutation result or a support loss happened meanwhile: this response is stale.
        if (mine !== epoch || generation !== listGeneration) return get().machines
        adopt(machines)
        return machines
      } catch (cause) {
        if (mine === epoch && generation === listGeneration) set({ listError: messageOf(cause) })
        return null
      } finally {
        if (mine === epoch && request === listRequest) set({ loading: false })
      }
    },
    async refreshStatus(id) {
      const mine = epoch
      const seq = (statusSeq.get(id) ?? 0) + 1
      statusSeq.set(id, seq)
      const current = () => mine === epoch && statusSeq.get(id) === seq && exists(id)
      try {
        const status = await machinesStatus(id)
        if (!current()) return null
        set((state) => {
          const { [id]: _error, ...errorById } = state.errorById
          // A failing verdict makes the cached agents out of date until a snapshot succeeds again.
          const failing = status.status === "auth-required" || status.status === "error"
          const staleById = failing && id in state.snapshotById ? { ...state.staleById, [id]: true } : state.staleById
          return { statusById: { ...state.statusById, [id]: status }, errorById, staleById }
        })
        if (status.status === "reachable") set({ recoverSignal: { id, seq: ++recoverSeq } })
        return status
      } catch (cause) {
        // A busy manager is not a machine failure; stay silent.
        if (messageOf(cause).startsWith("machines-busy") || !current()) return null
        set((state) => ({ errorById: { ...state.errorById, [id]: messageOf(cause) } }))
        return null
      }
    },
    async refreshSnapshot(id) {
      const mine = epoch
      const seq = (snapshotSeq.get(id) ?? 0) + 1
      snapshotSeq.set(id, seq)
      const current = () => mine === epoch && snapshotSeq.get(id) === seq && exists(id)
      try {
        const snapshot = await machinesAgents(id)
        // Removed, superseded by a newer call, or the store was reset meanwhile.
        if (!current()) return { ok: true, code: null, stale: true }
        set((state) => {
          const { [id]: _error, ...errorById } = state.errorById
          const { [id]: _stale, ...staleById } = state.staleById
          // A successful snapshot supersedes an older manual status check (auth-required / error).
          const { [id]: _status, ...statusById } = state.statusById
          return { snapshotById: { ...state.snapshotById, [id]: snapshot }, errorById, staleById, statusById }
        })
        return { ok: true, code: null }
      } catch (cause) {
        const raw = messageOf(cause)
        // A busy manager is not a machine failure; keep everything as is.
        if (raw.startsWith("machines-busy")) return { ok: false, code: "machines-busy" }
        const code = parseMachineError(raw).code
        if (current()) {
          set((state) => ({
            errorById: { ...state.errorById, [id]: raw },
            staleById: id in state.snapshotById ? { ...state.staleById, [id]: true } : state.staleById
          }))
        }
        return current() ? { ok: false, code } : { ok: false, code, stale: true }
      }
    },
    rename: (id, label) => mutate(() => machinesRename(id, label)),
    setEnabled: (id, enabled) => mutate(() => machinesSetEnabled(id, enabled)),
    remove: (id) => mutate(() => machinesRemove(id)),
    requestRefresh(force = true) { set((state) => ({ refreshNonce: state.refreshNonce + 1, refreshForce: force })) },
    reset() {
      invalidate()
      capsSeq += 1
      capsLatest = Promise.resolve(null)
      statusSeq.clear()
      snapshotSeq.clear()
      set({ ...initial })
    }
  }
})

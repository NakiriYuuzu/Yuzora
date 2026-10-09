import { useEffect } from "react"
import { isWindowsPlatform } from "@/lib/platform"
import { useMachinesStore } from "@/state/machinesStore"
import {
  HERDR_BINARY_SOURCE_CHANGED_EVENT, MACHINES_CAPABILITY_RETRY_MS, MACHINES_CONCURRENCY, machineBackoffDelay, machinePollIntervals
} from "./machinesPolicy"

interface MachineBackoff { failures: number; nextAt: number; authBlocked: boolean }

/**
 * Polls enabled HERDR machines' agent snapshots. Rounds never overlap, only a few machines are
 * queried at once, failures back off exponentially and `auth-required` waits for the user.
 */
function createMachinesPoller() {
  const windows = isWindowsPlatform()
  const intervals = machinePollIntervals(windows)
  const state = new Map<string, MachineBackoff>()
  /** Bumped by `recover(id)`; an older in-flight result for that machine must not re-block it. */
  const recovery = new Map<string, number>()
  let timer: ReturnType<typeof setTimeout> | null = null
  let running = false
  let rerun = false
  let stopped = false
  const hidden = () => document.visibilityState === "hidden"
  // A loaded empty catalog only needs to notice a machine added outside Yuzora (the
  // official CLI); a failed one retries at the normal pace.
  const idleCatalog = () => {
    const { machines, listError } = useMachinesStore.getState()
    return machines.length === 0 && listError === null
  }
  const base = () => (hidden() || idleCatalog() ? intervals.hidden : intervals.visible)

  /** Forget machines that vanished or were disabled, so re-enabling one polls it again. */
  function prune(machines: readonly { id: string; enabled: boolean }[]) {
    for (const id of [...state.keys()]) if (!machines.some((machine) => machine.id === id && machine.enabled)) state.delete(id)
  }

  /** Query one machine and fold the outcome into its backoff entry. */
  async function fetchOne(id: string) {
    const token = recovery.get(id) ?? 0
    const result = await useMachinesStore.getState().refreshSnapshot(id)
    if (stopped || (recovery.get(id) ?? 0) !== token) return
    if (result.stale || result.code === "machines-busy") return
    if (result.ok) { state.delete(id); return }
    const failures = (state.get(id)?.failures ?? 0) + 1
    state.set(id, {
      failures,
      nextAt: Date.now() + machineBackoffDelay(intervals.visible, failures),
      authBlocked: result.code === "machines-auth-required"
    })
  }

  async function round(force: boolean) {
    const store = useMachinesStore.getState()
    if (!store.capabilities?.supported) return
    if (force) state.clear()
    await store.refreshList()
    if (stopped) return
    // Hidden window: keep only the low-frequency list refresh.
    if (hidden() && !force) return
    const machines = useMachinesStore.getState().machines
    prune(machines)
    const now = Date.now()
    const due = machines.filter((machine) => {
      if (!machine.enabled) return false
      const entry = state.get(machine.id)
      return !entry || (!entry.authBlocked && now >= entry.nextAt)
    })
    let cursor = 0
    const worker = async () => {
      while (!stopped && cursor < due.length) {
        const machine = due[cursor++]
        // A mutation may have removed or disabled it since the due list was computed.
        if (!useMachinesStore.getState().machines.some((entry) => entry.id === machine.id && entry.enabled)) continue
        await fetchOne(machine.id)
      }
    }
    await Promise.all(Array.from({ length: Math.min(MACHINES_CONCURRENCY, due.length) }, worker))
  }

  let forceNext = false
  async function tick() {
    if (stopped) return
    running = true
    const force = forceNext
    forceNext = false
    try { await round(force) } catch { /* store actions already record failures */ }
    running = false
    if (stopped) return
    if (rerun) { rerun = false; void tick(); return }
    schedule()
  }
  function schedule() {
    if (timer) clearTimeout(timer)
    timer = setTimeout(() => { timer = null; void tick() }, base())
  }
  return {
    start() { stopped = false; void tick() },
    /** Run a round now (or right after the current one). `force` also clears auth/backoff blocks. */
    kick(force = false) {
      if (stopped) return
      if (force) forceNext = true
      if (running) { rerun = true; return }
      if (timer) { clearTimeout(timer); timer = null }
      void tick()
    },
    prune,
    /** A manual status check found the machine reachable: lift only its auth/backoff block and fetch once. */
    recover(id: string) {
      if (stopped) return
      recovery.set(id, (recovery.get(id) ?? 0) + 1)
      state.delete(id)
      if (useMachinesStore.getState().machines.some((machine) => machine.id === id && machine.enabled)) void fetchOne(id)
    },
    /** Re-arm the timer with the current visibility's interval. */
    reschedule() { if (!stopped && !running) schedule() },
    stop() {
      stopped = true
      if (timer) clearTimeout(timer)
      timer = null
    }
  }
}

export function MachinesBridge() {
  useEffect(() => {
    let poller: ReturnType<typeof createMachinesPoller> | null = null
    let capabilityRetry: ReturnType<typeof setTimeout> | null = null
    let disposed = false
    const sync = () => {
      const { capabilities, capabilitiesError } = useMachinesStore.getState()
      // Every supported runtime keeps polling: an empty or failed catalog at the slow
      // interval, so machines added through the official CLI still show up.
      const shouldPoll = Boolean(capabilities?.supported)
      if (shouldPoll && !poller) { poller = createMachinesPoller(); poller.start() }
      else if (!shouldPoll && poller) { poller.stop(); poller = null }
      // A probe that failed or found no parsable version (timeout, spawn failure, missing binary),
      // or a supported runtime whose subcommand probes did not finish, is retried slowly; a
      // confirmed old version and a remote-only source stay idle.
      const incomplete = capabilitiesError !== null || (capabilities !== null && !capabilities.supported
        && capabilities.version === null && capabilities.reason !== "machines-local-only")
        || capabilities?.probesComplete === false
      if (incomplete && !capabilityRetry && !disposed) {
        capabilityRetry = setTimeout(() => { capabilityRetry = null; void bootstrap() }, MACHINES_CAPABILITY_RETRY_MS)
      } else if (!incomplete && capabilityRetry) { clearTimeout(capabilityRetry); capabilityRetry = null }
    }
    const bootstrap = async () => {
      const capabilities = await useMachinesStore.getState().loadCapabilities()
      if (disposed) return
      if (capabilities?.supported) await useMachinesStore.getState().refreshList()
      if (disposed) return
      sync()
    }
    void bootstrap()
    let nonce = useMachinesStore.getState().refreshNonce
    const unsubscribe = useMachinesStore.subscribe((state, previous) => {
      sync()
      // A disable and re-enable can both land between two rounds; prune on every list change.
      if (state.machines !== previous.machines) poller?.prune(state.machines)
      if (state.recoverSignal && state.recoverSignal !== previous.recoverSignal) poller?.recover(state.recoverSignal.id)
      if (state.refreshNonce !== nonce) { nonce = state.refreshNonce; poller?.kick(state.refreshForce) }
    })
    const onVisibility = () => {
      if (document.visibilityState === "hidden") poller?.reschedule()
      else poller?.kick()
    }
    const onBinarySource = () => { void bootstrap() }
    document.addEventListener("visibilitychange", onVisibility)
    window.addEventListener(HERDR_BINARY_SOURCE_CHANGED_EVENT, onBinarySource)
    return () => {
      disposed = true
      unsubscribe()
      poller?.stop()
      if (capabilityRetry) clearTimeout(capabilityRetry)
      document.removeEventListener("visibilitychange", onVisibility)
      window.removeEventListener(HERDR_BINARY_SOURCE_CHANGED_EVENT, onBinarySource)
    }
  }, [])
  return null
}

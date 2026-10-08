import { useEffect } from "react"
import { isWindowsPlatform } from "@/lib/platform"
import { useMachinesStore } from "@/state/machinesStore"
import {
  HERDR_BINARY_SOURCE_CHANGED_EVENT, MACHINES_CONCURRENCY, machineBackoffDelay, machinePollIntervals
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
  let timer: ReturnType<typeof setTimeout> | null = null
  let running = false
  let rerun = false
  let stopped = false
  const hidden = () => document.visibilityState === "hidden"
  const base = () => (hidden() ? intervals.hidden : intervals.visible)

  async function round(force: boolean) {
    const store = useMachinesStore.getState()
    if (!store.capabilities?.supported) return
    if (force) state.clear()
    await store.refreshList()
    // Hidden window: keep only the low-frequency list refresh.
    if (hidden() && !force) return
    const machines = useMachinesStore.getState().machines
    for (const id of [...state.keys()]) if (!machines.some((machine) => machine.id === id)) state.delete(id)
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
        const result = await useMachinesStore.getState().refreshSnapshot(machine.id)
        if (result.code === "machines-busy") continue
        if (result.ok) { state.delete(machine.id); continue }
        const failures = (state.get(machine.id)?.failures ?? 0) + 1
        state.set(machine.id, {
          failures,
          nextAt: Date.now() + machineBackoffDelay(intervals.visible, failures),
          authBlocked: result.code === "machines-auth-required"
        })
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
    const sync = () => {
      const { capabilities, machines } = useMachinesStore.getState()
      const shouldPoll = Boolean(capabilities?.supported) && machines.length > 0
      if (shouldPoll && !poller) { poller = createMachinesPoller(); poller.start() }
      else if (!shouldPoll && poller) { poller.stop(); poller = null }
    }
    const bootstrap = async () => {
      const capabilities = await useMachinesStore.getState().loadCapabilities()
      if (capabilities?.supported) await useMachinesStore.getState().refreshList()
      sync()
    }
    void bootstrap()
    let nonce = useMachinesStore.getState().refreshNonce
    const unsubscribe = useMachinesStore.subscribe((state) => {
      sync()
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
      unsubscribe()
      poller?.stop()
      document.removeEventListener("visibilitychange", onVisibility)
      window.removeEventListener(HERDR_BINARY_SOURCE_CHANGED_EVENT, onBinarySource)
    }
  }, [])
  return null
}

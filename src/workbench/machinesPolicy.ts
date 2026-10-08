export const MACHINES_BACKOFF_CAP_MS = 5 * 60_000
export const MACHINES_CONCURRENCY = 2
/** Dispatched on window after the HERDR binary source changes; reloads capabilities. */
export { HERDR_BINARY_SOURCE_CHANGED_EVENT } from "@/lib/herdrBinarySourceEvents"

/** Windows has no SSH ControlMaster, so every machine call pays a full handshake: poll less. */
export function machinePollIntervals(windows: boolean) {
  return windows ? { visible: 30_000, hidden: 120_000 } : { visible: 15_000, hidden: 60_000 }
}

export function machineBackoffDelay(baseMs: number, failures: number) {
  return Math.min(baseMs * 2 ** failures, MACHINES_BACKOFF_CAP_MS)
}

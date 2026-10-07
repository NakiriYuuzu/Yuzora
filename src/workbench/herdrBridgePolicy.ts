import type { HerdrCapabilities } from "@/lib/herdrTypes"

/**
 * Poll until a real events subscriber is healthy.
 * Event capability alone is not delivery; only a live subscription suppresses polling.
 */
export const HERDR_HEALTHY_SNAPSHOT_FALLBACK_MS = 12_000

/** Events stay live; only the periodic health/discovery fallback backs off. */
export function startHerdrVisibilityPolling(poll: () => void): () => void {
  let timer: ReturnType<typeof setInterval>
  const schedule = () => {
    clearInterval(timer)
    timer = setInterval(poll, document.visibilityState === "hidden" ? 30_000 : 4000)
  }
  const onVisibility = () => {
    schedule()
    if (document.visibilityState !== "hidden") poll()
  }
  schedule()
  document.addEventListener("visibilitychange", onVisibility)
  return () => {
    clearInterval(timer)
    document.removeEventListener("visibilitychange", onVisibility)
  }
}

export function shouldPollHerdrSnapshots(
  capabilities: HerdrCapabilities | null,
  eventsHealthy = false,
  elapsedSinceSuccessMs = 0,
  healthyFallbackMs = HERDR_HEALTHY_SNAPSHOT_FALLBACK_MS
): boolean {
  if (!capabilities?.api.snapshot) return false
  if (eventsHealthy && capabilities.events.status === "available") {
    return elapsedSinceSuccessMs >= healthyFallbackMs
  }
  return true
}

/**
 * Protocol 19 advertises worktree selectors, but its subscription-event schema
 * does not enumerate their envelopes. Keep a low-frequency authoritative list
 * fallback even while the event stream is healthy.
 */
export function shouldRefreshWorktreeInventory(
  capabilities: HerdrCapabilities | null,
  elapsedMs: number,
  fallbackMs: number
): boolean {
  return Boolean(capabilities?.api.worktreeList && elapsedMs >= fallbackMs)
}

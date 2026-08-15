import type { HerdrCapabilities } from "@/lib/herdrTypes"

/**
 * Poll until a real events subscriber is healthy.
 * Event capability alone is not delivery; only a live subscription suppresses polling.
 */
export const HERDR_HEALTHY_SNAPSHOT_FALLBACK_MS = 12_000

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

/** Server clock minus the phone's, from `hello`/`session.ping` serverTime. */
export function clockOffset(serverTime: string, now = Date.now()): number {
  const server = Date.parse(serverTime)
  return Number.isNaN(server) ? 0 : server - now
}

/** Whole seconds left until a server timestamp, never negative. */
export function secondsUntil(until: string, offset: number, now = Date.now()): number {
  const end = Date.parse(until)
  if (Number.isNaN(end)) return 0
  return Math.max(0, Math.floor((end - (now + offset)) / 1000))
}

export function formatCountdown(seconds: number): string {
  return `${Math.floor(seconds / 60)}:${String(seconds % 60).padStart(2, "0")}`
}

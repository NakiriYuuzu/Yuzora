/** "1h 30m" or "45m" until the gateway's deadline; null when it has none. */
export function companionRemaining(enableUntil: string | null, now: number): string | null {
  if (!enableUntil) return null
  const minutes = Math.max(0, Math.ceil((Date.parse(enableUntil) - now) / 60_000))
  return minutes >= 60 ? `${Math.floor(minutes / 60)}h ${minutes % 60}m` : `${minutes}m`
}

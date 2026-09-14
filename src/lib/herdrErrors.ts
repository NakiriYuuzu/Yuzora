/** Classify boundary errors without treating an ambiguous mutation timeout as a retry. */
export type HerdrErrorKind = 'unknown-capability' | 'unsupported' | 'permission-denied' | 'busy' | 'disconnected' | 'timeout' | 'temporary'
export function herdrErrorKind(error: unknown): HerdrErrorKind {
  const message = error instanceof Error ? error.message : String(error)
  if (/^(host-request-limit|host-request-wait-timeout|host-call-limit|host-call-wait-timeout)$/.test(message)) return 'busy'
  if (/permission.denied|forbidden|not.authorized|not.controller|access.denied/i.test(message)) return 'permission-denied'
  if (/disconnected|connection changed|connector.*closed|session.*not running|AbortError|scroll-cancelled/i.test(message)) return 'disconnected'
  if (/unknown.method|method.not.found|unsupported.method|herdr (pane\.(get|scroll)|workspace\.move(_block)?) unavailable/i.test(message)) return 'unsupported'
  if (/capabilit.*(unknown|pending)|schema.*unavailable/i.test(message)) return 'unknown-capability'
  if (/timeout|timed out/i.test(message)) return 'timeout'
  return 'temporary'
}

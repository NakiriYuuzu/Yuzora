/** Classify boundary errors without treating an ambiguous mutation timeout as a retry. */
export type HerdrErrorKind = 'unknown-capability' | 'unsupported' | 'permission-denied' | 'busy' | 'disconnected' | 'timeout' | 'temporary'
export function herdrErrorKind(error: unknown): HerdrErrorKind {
  const message = error instanceof Error ? error.message : String(error)
  if (/^(host-request-limit|host-request-wait-timeout|host-call-limit|host-call-wait-timeout|host-stream-open-limit|host-stream-open-wait-timeout|stream-closed-or-busy)$/.test(message)) return 'busy'
  if (/permission.denied|forbidden|not.authorized|not.controller|access.denied/i.test(message)) return 'permission-denied'
  if (/disconnected|connection changed|connector.*closed|session.*not running|AbortError|scroll-cancelled/i.test(message)) return 'disconnected'
  if (/unknown.method|method.not.found|unsupported.method|herdr (pane\.(get|scroll)|workspace\.move(_block)?) unavailable/i.test(message)) return 'unsupported'
  if (/capabilit.*(unknown|pending)|schema.*unavailable/i.test(message)) return 'unknown-capability'
  if (/timeout|timed out/i.test(message)) return 'timeout'
  return 'temporary'
}

/**
 * An automatic connector (re)open may retry with backoff unless the runtime
 * refused the operation itself. Remote helpers bound concurrent openings and
 * streams, so a burst of Sessions or panes is a transient condition.
 */
export function isRetryableHerdrConnectError(error: unknown): boolean {
  const kind = herdrErrorKind(error)
  return kind !== 'permission-denied' && kind !== 'unsupported' && kind !== 'unknown-capability'
}

export interface DescribedHerdrError {
  /** Kebab-case error code, or null when the raw text is not a known shape. */
  code: string | null
  /** Technical remainder after the code (long runtime diagnostics, paths, OS errors). */
  detail: string | null
  /** Localized, actionable headline; falls back to the raw text for unknown codes. */
  message: string
  raw: string
}

type Translate = (key: string, options?: Record<string, unknown>) => string

const CODE_PATTERN = /^([a-z][a-z0-9]*(?:-[a-z0-9]+)+)(?::\s*(.*))?$/s

/** Backend sentences predating the code convention; matched exactly (or by prefix when it carries a path). */
const LEGACY_EXACT: Record<string, string> = {
  "Herdr was not found on PATH": "herdr-not-found-on-path",
  "herdr binary is unavailable for startup": "herdr-binary-unavailable",
  "herdr config directory is not configured": "herdr-config-dir-not-configured"
}
const LEGACY_PREFIX: Array<[string, string]> = [["herdr binary override is not executable:", "herdr-custom-path-not-executable"]]

function rawText(error: unknown): string {
  const text = error instanceof Error ? error.message : String(error)
  return text.replace(/^Error:\s*/, "").trim()
}

/** Map a backend error string to a localized message plus collapsible technical detail. */
export function describeHerdrError(error: unknown, t: Translate): DescribedHerdrError {
  const raw = rawText(error)
  let code: string | null = null
  let detail: string | null = null
  if (raw in LEGACY_EXACT) code = LEGACY_EXACT[raw]
  else {
    const legacy = LEGACY_PREFIX.find(([prefix]) => raw.startsWith(prefix))
    if (legacy) { code = legacy[1]; detail = raw.slice(legacy[0].length).trim() || null }
  }
  if (!code) {
    const match = CODE_PATTERN.exec(raw)
    if (match) { code = match[1]; detail = match[2]?.trim() || null }
  }
  if (!code) return { code: null, detail: null, message: raw, raw }
  const key = `herdrErrors:${code}`
  const translated = t(key, { detail: detail ?? "", defaultValue: "" })
  if (!translated || translated === key || translated === code) return { code: null, detail: null, message: raw, raw }
  return { code, detail, message: translated, raw }
}

/** Stable page identity helpers for Herdr terminal pages. */

export const HERDR_PAGE_SCHEME = "yuzora://herdr/"

export function herdrPagePath(herdrSessionId: string, terminalId: string): string {
  return `${HERDR_PAGE_SCHEME}${encodeURIComponent(herdrSessionId)}/${encodeURIComponent(terminalId)}`
}

export function parseHerdrPagePath(
  path: string
): { herdrSessionId: string; terminalId: string } | null {
  if (!path.startsWith(HERDR_PAGE_SCHEME)) return null
  const rest = path.slice(HERDR_PAGE_SCHEME.length)
  const slash = rest.indexOf("/")
  if (slash <= 0 || slash === rest.length - 1) return null
  try {
    return {
      herdrSessionId: decodeURIComponent(rest.slice(0, slash)),
      terminalId: decodeURIComponent(rest.slice(slash + 1))
    }
  } catch {
    return null
  }
}

export function isHerdrPagePath(path: string): boolean {
  return parseHerdrPagePath(path) !== null
}

/** Composite attachment key so sibling leaves on one page never overwrite. */
export function herdrAttachmentKey(pagePath: string, paneKey: string): string {
  return `${pagePath}::${paneKey}`
}

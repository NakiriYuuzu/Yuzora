import type { HerdrRuntimeTarget } from "./herdrTypes"
import {
  HERDR_NATIVE_RUNTIME_TARGET,
  herdrRuntimeKey,
  normalizeHerdrRuntimeTarget
} from "./herdrRuntime"

/** Stable page identity helpers for Herdr terminal pages. */

export const HERDR_PAGE_SCHEME = "yuzora://herdr/"
const HERDR_PAGE_V2 = "v2"

export interface ParsedHerdrPagePath {
  herdrSessionId: string
  terminalId: string
  /** Legacy v1 paths resolve to Native rather than guessing a remote runtime. */
  runtimeTarget: HerdrRuntimeTarget
  legacy: boolean
}

/**
 * Native keeps its legacy path so persisted pages and Native user-visible
 * identity stay stable. Remote targets always use v2 and include their target.
 */
export function herdrPagePath(
  herdrSessionId: string,
  terminalId: string,
  runtimeTarget?: HerdrRuntimeTarget | null
): string {
  const target = normalizeHerdrRuntimeTarget(runtimeTarget)
  if (target.kind === "native") {
    return `${HERDR_PAGE_SCHEME}${encodeURIComponent(herdrSessionId)}/${encodeURIComponent(terminalId)}`
  }
  return `${HERDR_PAGE_SCHEME}${HERDR_PAGE_V2}/${encodeURIComponent(herdrRuntimeKey(target))}/${encodeURIComponent(herdrSessionId)}/${encodeURIComponent(terminalId)}`
}

export function parseHerdrPagePath(path: string): ParsedHerdrPagePath | null {
  if (!path.startsWith(HERDR_PAGE_SCHEME)) return null
  const rest = path.slice(HERDR_PAGE_SCHEME.length)
  const parts = rest.split("/")
  try {
    if (parts.length === 4 && parts[0] === HERDR_PAGE_V2) {
      const runtimeKey = decodeURIComponent(parts[1] ?? "")
      const herdrSessionId = decodeURIComponent(parts[2] ?? "")
      const terminalId = decodeURIComponent(parts[3] ?? "")
      if (!runtimeKey || !herdrSessionId || !terminalId) return null
      const runtimeTarget = parseHerdrRuntimeKey(runtimeKey)
      if (!runtimeTarget) return null
      return { herdrSessionId, terminalId, runtimeTarget, legacy: false }
    }

    // v1: yuzora://herdr/<session>/<terminal>. Never infer WSL from a string.
    if (parts.length !== 2 || !parts[0] || !parts[1]) return null
    return {
      herdrSessionId: decodeURIComponent(parts[0]),
      terminalId: decodeURIComponent(parts[1]),
      runtimeTarget: HERDR_NATIVE_RUNTIME_TARGET,
      legacy: true
    }
  } catch {
    return null
  }
}

function parseHerdrRuntimeKey(key: string): HerdrRuntimeTarget | null {
  if (key === "native") return HERDR_NATIVE_RUNTIME_TARGET
  if (!key.startsWith("wsl:")) return null
  const distro = decodeURIComponent(key.slice("wsl:".length))
  return distro.trim() ? { kind: "wsl", distro } : null
}

export function isHerdrPagePath(path: string): boolean {
  return parseHerdrPagePath(path) !== null
}

/** Composite attachment key so sibling leaves on one page never overwrite. */
export function herdrAttachmentKey(pagePath: string, paneKey: string): string {
  return `${pagePath}::${paneKey}`
}

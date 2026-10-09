import type { HerdrRuntimeSelection } from "./herdrTypes"

const QUOTES = new Set(['"', "'", "\u201c", "\u2018"])
const CLOSERS: Record<string, string> = { '"': '"', "'": "'", "\u201c": "\u201d", "\u2018": "\u2019" }

/**
 * Normalise a pasted HERDR executable path: trim, then strip exactly one pair of
 * matching surrounding quotes (Explorer "Copy as path" yields "C:\\a b\\herdr.exe").
 * Inner spaces and unpaired quotes are preserved so half-typed input is never rewritten.
 * Must stay in step with the Rust normaliser; shared vectors live in herdrPath.test.ts.
 */
export function sanitizeCustomPath(raw: string): string {
  const trimmed = raw.trim()
  if (trimmed.length < 2) return trimmed
  const first = trimmed[0]
  if (QUOTES.has(first) && trimmed[trimmed.length - 1] === CLOSERS[first]) return trimmed.slice(1, -1).trim()
  return trimmed
}

export function sanitizeSelection(selection: HerdrRuntimeSelection): HerdrRuntimeSelection {
  if (selection.source !== "custom" || selection.customPath === undefined) return selection
  return { ...selection, customPath: sanitizeCustomPath(selection.customPath) }
}

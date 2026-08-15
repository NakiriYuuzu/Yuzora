/* eslint-disable no-control-regex -- ANSI parsing intentionally matches terminal control sequences. */
import type { CSSProperties } from "react"

export const MAX_ANSI_INPUT_BYTES = 512 * 1024
export const MAX_ANSI_CODE_POINTS = 512 * 1024
export const MAX_ANSI_SEGMENTS = 10_000

const ANSI_COLORS = [
  "#1d1f21",
  "#cc6666",
  "#b5bd68",
  "#f0c674",
  "#81a2be",
  "#b294bb",
  "#8abeb7",
  "#c5c8c6",
  "#969896",
  "#de935f",
  "#b5bd68",
  "#f0c674",
  "#81a2be",
  "#b294bb",
  "#8abeb7",
  "#ffffff"
]

const OSC_PATTERN = new RegExp(
  "\\u001b\\][^\\u0007]*(?:\\u0007|\\u001b\\\\)",
  "g"
)
const NON_SGR_CSI_PATTERN = new RegExp(
  "\\u001b\\[[0-?]*[ -/]*(?!m)[@-~]",
  "g"
)
const SGR_PATTERN = new RegExp("\\u001b\\[([0-9;]*)m", "g")

export interface AnsiSegment {
  text: string
  style: CSSProperties
}

export interface AnsiParseResult {
  segments: AnsiSegment[]
  truncated: boolean
  tooLarge: boolean
}

function colorFromIndex(index: number): string | undefined {
  if (index < 0 || index > 255) return undefined
  if (index < 16) return ANSI_COLORS[index]
  if (index < 232) {
    const value = index - 16
    const component = (part: number) => (part === 0 ? 0 : 55 + part * 40)
    const red = component(Math.floor(value / 36))
    const green = component(Math.floor((value % 36) / 6))
    const blue = component(value % 6)
    return `rgb(${red}, ${green}, ${blue})`
  }
  const gray = 8 + (index - 232) * 10
  return `rgb(${gray}, ${gray}, ${gray})`
}

function applySgr(style: CSSProperties, values: number[]): CSSProperties {
  const next = { ...style }
  for (let index = 0; index < values.length; index += 1) {
    const code = values[index] ?? 0
    if (code === 0) {
      for (const key of Object.keys(next) as Array<keyof CSSProperties>) delete next[key]
    } else if (code === 1) {
      next.fontWeight = 700
    } else if (code === 2) {
      next.opacity = 0.7
    } else if (code === 3) {
      next.fontStyle = "italic"
    } else if (code === 4) {
      next.textDecoration = "underline"
    } else if (code === 22) {
      delete next.fontWeight
      delete next.opacity
    } else if (code === 23) {
      delete next.fontStyle
    } else if (code === 24) {
      delete next.textDecoration
    } else if (code >= 30 && code <= 37) {
      next.color = ANSI_COLORS[code - 30]
    } else if (code === 39) {
      delete next.color
    } else if (code >= 40 && code <= 47) {
      next.backgroundColor = ANSI_COLORS[code - 40]
    } else if (code === 49) {
      delete next.backgroundColor
    } else if (code >= 90 && code <= 97) {
      next.color = ANSI_COLORS[8 + code - 90]
    } else if (code >= 100 && code <= 107) {
      next.backgroundColor = ANSI_COLORS[8 + code - 100]
    } else if ((code === 38 || code === 48) && values[index + 1] === 5) {
      const color = colorFromIndex(values[index + 2] ?? -1)
      if (color) {
        if (code === 38) next.color = color
        else next.backgroundColor = color
      }
      index += 2
    } else if ((code === 38 || code === 48) && values[index + 1] === 2) {
      const [red, green, blue] = values.slice(index + 2, index + 5)
      if (
        [red, green, blue].every(
          (part) => Number.isInteger(part) && part >= 0 && part <= 255
        )
      ) {
        const color = `rgb(${red}, ${green}, ${blue})`
        if (code === 38) next.color = color
        else next.backgroundColor = color
      }
      index += 4
    }
  }
  return next
}

function utf8BytesForCodePoint(codePoint: number): number {
  if (codePoint <= 0x7f) return 1
  if (codePoint <= 0x7ff) return 2
  if (codePoint <= 0xffff) return 3
  return 4
}

function boundAnsiInput(input: string): { text: string; tooLarge: boolean } {
  let bytes = 0
  let codePoints = 0
  let end = 0
  for (const char of input) {
    const codePoint = char.codePointAt(0) ?? 0
    const charBytes = utf8BytesForCodePoint(codePoint)
    if (bytes + charBytes > MAX_ANSI_INPUT_BYTES || codePoints + 1 > MAX_ANSI_CODE_POINTS) {
      return { text: input.slice(0, end), tooLarge: true }
    }
    bytes += charBytes
    codePoints += 1
    end += char.length
  }
  return { text: input, tooLarge: false }
}

function stylesEqual(left: CSSProperties, right: CSSProperties): boolean {
  return (
    left.color === right.color &&
    left.backgroundColor === right.backgroundColor &&
    left.fontWeight === right.fontWeight &&
    left.fontStyle === right.fontStyle &&
    left.textDecoration === right.textDecoration &&
    left.opacity === right.opacity
  )
}

function pushSegment(
  segments: AnsiSegment[],
  text: string,
  style: CSSProperties
): boolean {
  if (!text) return true
  const last = segments[segments.length - 1]
  if (last && stylesEqual(last.style, style)) {
    last.text += text
    return true
  }
  if (segments.length >= MAX_ANSI_SEGMENTS) return false
  segments.push({ text, style: { ...style } })
  return true
}

export function parseAnsiSegments(input: string): AnsiParseResult {
  const bounded = boundAnsiInput(input)
  const sanitized = bounded.text.replace(OSC_PATTERN, "").replace(NON_SGR_CSI_PATTERN, "")
  const segments: AnsiSegment[] = []
  let style: CSSProperties = {}
  let cursor = 0
  let truncated = false
  for (const match of sanitized.matchAll(SGR_PATTERN)) {
    const index = match.index ?? cursor
    if (index > cursor) {
      if (!pushSegment(segments, sanitized.slice(cursor, index), style)) {
        truncated = true
        break
      }
    }
    if (truncated) break
    const values = match[1]
      ? match[1].split(";").map((value) => Number.parseInt(value || "0", 10))
      : [0]
    style = applySgr(style, values)
    cursor = index + match[0].length
  }
  if (!truncated && cursor < sanitized.length) {
    if (!pushSegment(segments, sanitized.slice(cursor), style)) {
      truncated = true
    }
  }
  return {
    segments,
    truncated,
    tooLarge: bounded.tooLarge
  }
}

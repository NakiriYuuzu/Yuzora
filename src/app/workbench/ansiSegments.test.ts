import { describe, expect, it } from "vitest"

import {
  MAX_ANSI_INPUT_BYTES,
  MAX_ANSI_SEGMENTS,
  parseAnsiSegments
} from "./ansiSegments"

describe("parseAnsiSegments", () => {
  it("keeps normal ANSI color and reset parity", () => {
    const { segments, truncated, tooLarge } = parseAnsiSegments(
      "\u001b[31mred\u001b[0m plain \u001b[1;32mbold-green"
    )
    expect(tooLarge).toBe(false)
    expect(truncated).toBe(false)
    expect(segments).toEqual([
      { text: "red", style: { color: "#cc6666" } },
      { text: " plain ", style: {} },
      { text: "bold-green", style: { fontWeight: 700, color: "#b5bd68" } }
    ])
  })

  it("coalesces adjacent identical styles", () => {
    const { segments } = parseAnsiSegments("\u001b[31mred\u001b[31mmore\u001b[31m!")
    expect(segments).toEqual([{ text: "redmore!", style: { color: "#cc6666" } }])
  })

  it("caps one-SGR-per-character input to the segment ceiling", () => {
    const count = MAX_ANSI_SEGMENTS + 50
    let input = ""
    for (let index = 0; index < count; index += 1) {
      input += `\u001b[${31 + (index % 6)}m${String.fromCharCode(97 + (index % 26))}`
    }
    const { segments, truncated, tooLarge } = parseAnsiSegments(input)
    expect(tooLarge).toBe(false)
    expect(truncated).toBe(true)
    expect(segments.length).toBeLessThanOrEqual(MAX_ANSI_SEGMENTS)
    expect(segments.length).toBe(MAX_ANSI_SEGMENTS)
  })

  it("marks huge plain text as tooLarge and keeps a bounded prefix", () => {
    const input = "a".repeat(MAX_ANSI_INPUT_BYTES + 32)
    const { segments, truncated, tooLarge } = parseAnsiSegments(input)
    expect(tooLarge).toBe(true)
    expect(truncated).toBe(false)
    expect(segments).toHaveLength(1)
    expect(segments[0]?.text.length).toBe(MAX_ANSI_INPUT_BYTES)
    expect(segments[0]?.text.startsWith("aaa")).toBe(true)
  })

  it("accepts input just below the byte cap as a single segment", () => {
    const input = "b".repeat(MAX_ANSI_INPUT_BYTES)
    const { segments, tooLarge } = parseAnsiSegments(input)
    expect(tooLarge).toBe(false)
    expect(segments).toEqual([{ text: input, style: {} }])
  })
})

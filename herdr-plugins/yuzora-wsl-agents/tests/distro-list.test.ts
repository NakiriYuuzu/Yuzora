import { describe, expect, it } from "vitest"
import { assertSafeDistroName, decodeWslListOutput } from "../lib/distro-list"

function utf16le(text: string, bom = false): Uint8Array {
  const units = [...text].map((ch) => ch.charCodeAt(0))
  const bytes = new Uint8Array((bom ? 2 : 0) + units.length * 2)
  let offset = 0
  if (bom) {
    bytes[0] = 0xff
    bytes[1] = 0xfe
    offset = 2
  }
  units.forEach((unit, index) => {
    bytes[offset + index * 2] = unit & 0xff
    bytes[offset + index * 2 + 1] = unit >> 8
  })
  return bytes
}

describe("WSL distro list decoding", () => {
  it("decodes UTF-16LE with BOM, NULs, and CRLF", () => {
    const bytes = utf16le("\uFEFFUbuntu\0\r\nDebian\r\n", true)
    expect(decodeWslListOutput(bytes)).toEqual(["Ubuntu", "Debian"])
  })

  it("decodes UTF-8 and empty output", () => {
    expect(decodeWslListOutput(new TextEncoder().encode("Ubuntu\nDebian\n"))).toEqual([
      "Ubuntu",
      "Debian"
    ])
    expect(decodeWslListOutput(new Uint8Array())).toEqual([])
  })

  it("rejects path-like distro names", () => {
    expect(() => assertSafeDistroName("../evil")).toThrow(/unsafe/)
    expect(() => assertSafeDistroName("Ubuntu/../../tmp")).toThrow(/unsafe/)
    expect(assertSafeDistroName("Ubuntu-20.04")).toBe("Ubuntu-20.04")
  })
})

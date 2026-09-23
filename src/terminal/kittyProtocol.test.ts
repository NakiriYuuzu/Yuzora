import { describe, expect, it, vi } from "vitest"
import { KittyGraphics, KittyStreamParser, type KittyToken } from "./kittyProtocol"

function target() {
  const reply = vi.fn(), dispose = vi.fn(), changed = vi.fn()
  const load = vi.fn(async (bytes: Uint8Array, width: number, height: number) => ({ image: bytes, width, height }))
  const graphics = new KittyGraphics({ load, dispose, changed, reply, cursor: () => ({ col: 4, row: 8 }) })
  return { graphics, load, reply, dispose }
}
function command(control: Record<string, string>, payload = ""): Extract<KittyToken, { type: "graphics" }> {
  return { type: "graphics", control, payload }
}
const pixel = btoa(String.fromCharCode(255, 12, 48, 255))

describe("HERDR inline graphics", () => {
  it.each([24, 32])("inflates bounded zlib uploads for %i-bit pixel data", async format => {
    const { graphics, load } = target()
    const raw = new Uint8Array(format === 24 ? [255, 12, 48] : [255, 12, 48, 255])
    const compressed = format === 24 ? "eJz7z2MAAANIATw=" : "eJz7z2PwHwAFgwI7"
    await graphics.accept(command({ a: "T", i: "9", s: "1", v: "1", f: String(format), o: "z" }, compressed))
    expect(load).toHaveBeenCalledWith(raw, 1, 1, format)
    expect(graphics.placements.size).toBe(1)
  })
  it("preserves ANSI and Unicode and frames Kitty across every possible byte boundary", () => {
    const stream = `中文\x1b[2;3H\x1b_Ga=T,i=9,f=32,s=1,v=1;${pixel}\x1b\\終わり`
    for (let split = 0; split <= stream.length; split++) {
      const parser = new KittyStreamParser()
      const tokens = [...parser.feed(stream.slice(0, split)), ...parser.feed(stream.slice(split))]
      expect(tokens.filter(t => t.type === "text").map(t => t.text).join("")).toBe("中文\x1b[2;3H終わり")
      expect(tokens.filter(t => t.type === "graphics")).toEqual([command({ a: "T", i: "9", f: "32", s: "1", v: "1" }, pixel)])
    }
  })
  it("rejects an unterminated oversized sequence without passing it into the terminal", () => {
    const parser = new KittyStreamParser()
    expect(() => parser.feed("\x1b_G" + "a".repeat(65537))).toThrow("chunk limit")
  })
  it("assembles uploads before painting and replaces the exact placement", async () => {
    const { graphics, load, reply } = target()
    await graphics.accept(command({ a: "T", i: "9", p: "4", f: "32", s: "1", v: "1", c: "2", r: "3", z: "-1", q: "2", m: "1" }, pixel.slice(0, 4)))
    expect(load).not.toHaveBeenCalled()
    await graphics.accept(command({ m: "0" }, pixel.slice(4)))
    expect(load).toHaveBeenCalledWith(new Uint8Array([255, 12, 48, 255]), 1, 1, 32)
    expect(graphics.placements.get("9:4")).toMatchObject({ col: 4, row: 8, cols: 2, rows: 3, z: -1 })
    await graphics.accept(command({ a: "p", i: "9", p: "4", c: "5", q: "2" }))
    expect(graphics.placements.size).toBe(1)
    expect(graphics.placements.get("9:4")?.cols).toBe(5)
    expect(reply).not.toHaveBeenCalled()
  })
  it("keeps anonymous uploads from replacing a client-numbered image", async () => {
    const { graphics, dispose } = target()
    await graphics.accept(command({ a: "t", f: "32", s: "1", v: "1" }, pixel))
    await graphics.accept(command({ a: "t", i: "1", f: "32", s: "1", v: "1" }, pixel))
    expect(graphics.images.size).toBe(2)
    expect(dispose).not.toHaveBeenCalled()
    await graphics.accept(command({ a: "t", i: String(2 ** 32), f: "32", s: "1", v: "1" }, pixel))
    expect(graphics.images.size).toBe(2)
  })
  it("retains image data when removing a placement and releases it on image deletion", async () => {
    const { graphics, dispose } = target()
    await graphics.accept(command({ a: "T", i: "9", p: "4", f: "32", s: "1", v: "1" }, pixel))
    await graphics.accept(command({ a: "d", d: "i", i: "9", p: "4" }))
    expect(graphics.placements.size).toBe(0)
    expect(graphics.images.size).toBe(1)
    await graphics.accept(command({ a: "p", i: "9", p: "5" }))
    await graphics.accept(command({ a: "d", d: "I", i: "9" }))
    expect(graphics.images.size).toBe(0)
    expect(dispose).toHaveBeenCalledTimes(1)
  })
  it("answers capability queries without allocating an image", async () => {
    const { graphics, load, reply } = target()
    await graphics.accept(command({ a: "q", i: "31", s: "1", v: "1", f: "32" }, pixel))
    expect(reply).toHaveBeenCalledWith("\x1b_Gi=31;OK\x1b\\")
    expect(load).not.toHaveBeenCalled()
  })
  it("rejects file transports, malformed pixels, oversized PNGs and invalid crops before drawing", async () => {
    const { graphics, load, reply } = target()
    await graphics.accept(command({ a: "t", i: "1", t: "f" }, btoa("/private/file")))
    expect(reply.mock.lastCall?.[0]).toContain("ENOTSUP")
    await graphics.accept(command({ a: "t", i: "2", s: "1", v: "2", f: "32" }, pixel))
    expect(reply.mock.lastCall?.[0]).toContain("EINVAL")
    const png = new Uint8Array([137,80,78,71,13,10,26,10,0,0,0,13,73,72,68,82,0,0,128,0,0,0,128,0])
    await graphics.accept(command({ a: "t", i: "3", f: "100" }, btoa(String.fromCharCode(...png))))
    expect(reply.mock.lastCall?.[0]).toContain("EFBIG")
    expect(load).not.toHaveBeenCalled()
    await graphics.accept(command({ a: "t", i: "4", s: "1", v: "1", f: "32" }, pixel))
    await graphics.accept(command({ a: "p", i: "4", x: "100" }))
    expect(graphics.placements.size).toBe(0)
  })
  it("disposes decoded images arriving after the view has closed", async () => {
    const dispose = vi.fn()
    let complete!: (value: { image: string; width: number; height: number }) => void
    const graphics = new KittyGraphics<string>({ load: () => new Promise(resolve => { complete = resolve }), dispose, reply: vi.fn(), changed: vi.fn(), cursor: () => ({ col: 0, row: 0 }) })
    const upload = graphics.accept(command({ a: "T", i: "9", s: "1", v: "1", f: "32" }, pixel))
    graphics.dispose(); complete({ image: "bitmap", width: 1, height: 1 }); await upload
    expect(graphics.images.size).toBe(0)
    expect(dispose).toHaveBeenCalledWith("bitmap")
  })
})

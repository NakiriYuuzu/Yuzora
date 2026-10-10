import { readdirSync, readFileSync } from "node:fs"
import { join, resolve } from "node:path"
import { describe, expect, it } from "vitest"

const css = readFileSync(resolve(__dirname, "src/terminal/xtermBlockGlyphs.css"), "utf8")
const ALLOWED = new Set(["background-image", "background-size", "background-position", "background-repeat"])
const rules = [...css.matchAll(/([^{}]+)\{([^{}]*)\}/g)].map(([, selector, body]) => ({
  selector: selector.replace(/\/\*[\s\S]*?\*\//g, "").trim(),
  props: body.split(";").map((d) => d.trim()).filter(Boolean).map((d) => d.slice(0, d.indexOf(":")).trim()),
}))

describe("xterm block glyph stylesheet", () => {
  it("has a rule for every code point in U+2580-U+259F", () => {
    for (let cp = 0x2580; cp <= 0x259f; cp++) {
      const selector = `.xterm .xterm-rows .xterm-block-${cp.toString(16)}`
      const matching = rules.filter((r) => r.selector === selector)
      expect(matching, selector).toHaveLength(1)
      expect(matching[0].props).toEqual(expect.arrayContaining(["background-image", "background-size", "background-position"]))
    }
  })

  it("only paints through background image layers, never the cell background colour", () => {
    expect(rules.length).toBe(33)
    for (const rule of rules) for (const prop of rule.props) expect(ALLOWED.has(prop), `${rule.selector} ${prop}`).toBe(true)
    expect(css.replace(/\/\*[\s\S]*?\*\//g, "")).not.toMatch(/background-color|background:/)
  })

  it("is loaded by every entrypoint that loads xterm.css, since block cells render blank without it", () => {
    const sources = (dir: string): string[] => readdirSync(resolve(__dirname, dir), { withFileTypes: true }).flatMap((entry) =>
      entry.isDirectory() ? sources(join(dir, entry.name)) : /\.tsx?$/.test(entry.name) ? [join(dir, entry.name)] : [])
    const entrypoints = [...sources("src"), ...sources("fixtures")].filter((file) => readFileSync(resolve(__dirname, file), "utf8").includes("@xterm/xterm/css/xterm.css"))
    expect(entrypoints.length).toBeGreaterThan(4)
    for (const file of entrypoints) expect(readFileSync(resolve(__dirname, file), "utf8"), file).toContain("terminal/xtermBlockGlyphs.css")
  })

  it("keeps image/size/position layer counts aligned for multi-quadrant glyphs", () => {
    const layers = (v: string) => v.split(/,(?![^(]*\))/).length
    const count = (cp: number, prop: string) => {
      const body = css.match(new RegExp(`xterm-block-${cp.toString(16)} \\{([^}]*)\\}`))![1]
      return layers(body.match(new RegExp(`${prop}: ([^;]*);`))![1])
    }
    for (const [cp, n] of [[0x2596, 1], [0x2599, 3], [0x259a, 2], [0x259b, 3], [0x259c, 3], [0x259e, 2], [0x259f, 3]] as const)
      for (const prop of ["background-image", "background-size", "background-position"]) expect(count(cp, prop), `${cp.toString(16)} ${prop}`).toBe(n)
  })
})

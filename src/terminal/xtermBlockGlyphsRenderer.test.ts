import { Terminal } from "@xterm/xterm"
import { afterEach, describe, expect, it } from "vitest"

const flush = () => new Promise((resolve) => window.setTimeout(resolve, 50))
const write = (term: Terminal, data: string) => new Promise<void>((resolve) => term.write(data, resolve))

describe("patched xterm DOM renderer: block element glyphs", () => {
  let term: Terminal | undefined
  afterEach(() => {
    term?.dispose()
    term = undefined
    document.body.replaceChildren()
  })

  async function open(data: string) {
    const container = document.createElement("div")
    Object.defineProperties(container, {
      clientWidth: { configurable: true, value: 800 },
      clientHeight: { configurable: true, value: 480 },
    })
    document.body.append(container)
    term = new Terminal({ cols: 40, rows: 6 })
    term.open(container)
    await write(term, data)
    await flush()
    return term.element!.querySelectorAll<HTMLElement>(".xterm-rows > div")
  }

  it("renders each block cell as its own span holding a space, and still merges normal text", async () => {
    const orange = "\x1b[38;2;215;119;87m"
    const rows = await open(`${orange} ▐▛███▜▌ \x1b[0m hello world`)
    const spans = [...rows[0].querySelectorAll<HTMLElement>("span")]
    const blocks = spans.filter((s) => s.classList.contains("xterm-block-glyph"))
    expect(blocks.map((s) => [...s.classList].find((c) => /^xterm-block-[0-9a-f]{4}$/.test(c)))).toEqual([
      "xterm-block-2590", "xterm-block-259b", "xterm-block-2588", "xterm-block-2588", "xterm-block-2588", "xterm-block-259c", "xterm-block-258c",
    ])
    for (const block of blocks) {
      expect(block.textContent).toBe(" ")
      expect(block.style.color).toBe("rgb(215, 119, 87)")
    }
    expect(spans.some((s) => s.textContent?.includes("hello world"))).toBe(true)
  })

  it("keeps non-block characters such as box drawing in merged text runs", async () => {
    const rows = await open("─│abc▀")
    const spans = [...rows[0].querySelectorAll<HTMLElement>("span")]
    // the trailing blank span is the cursor cell
    expect(spans.map((s) => s.textContent)).toEqual(["─│abc", " ", " "])
    expect(spans[1].className).toContain("xterm-block-2580")
    expect(spans[0].className).not.toContain("xterm-block")
  })

  it("keeps background colour and inverse handling on block cells", async () => {
    const rows = await open("\x1b[48;2;1;2;3m▖\x1b[0m\x1b[7m▗\x1b[0m")
    const [bg, inv] = [...rows[0].querySelectorAll<HTMLElement>("span.xterm-block-glyph")]
    expect(bg.style.backgroundColor).toBe("rgb(1, 2, 3)")
    expect(bg.className).toContain("xterm-block-2596")
    expect(inv.className).toContain("xterm-bg-257")
    expect(inv.className).toContain("xterm-block-2597")
  })

  it("keeps concealed (SGR 8) block cells invisible", async () => {
    const rows = await open("\x1b[8m█\x1b[0m")
    expect(rows[0].querySelector(".xterm-block-glyph")).toBeNull()
  })
})

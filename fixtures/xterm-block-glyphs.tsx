/** Real DOM renderer acceptance page for block elements (U+2580-U+259F). No IPC, sessions or external data. */
import { useEffect, useRef, useState } from "react"
import { createRoot } from "react-dom/client"
import { Terminal } from "@xterm/xterm"
import { TERMINAL_FONTS, terminalFontStack, type TerminalFontFamily } from "../src/terminal/terminalFonts"
import { buildXtermTheme, xtermMinimumContrastRatio } from "../src/terminal/xtermTheme"
import "@xterm/xterm/css/xterm.css"
import "../src/terminal/xtermBlockGlyphs.css"
import "../src/styles.css"

const BODY = "\x1b[38;2;215;119;87m"
const EYES = "\x1b[38;2;0;0;0;48;2;215;119;87m"
const RESET = "\x1b[0m"
// Claude Code "Clawd" frames: body rgb(215,119,87), eyes are black-on-body cells.
const MASCOT = [
  ` ${BODY}▐${EYES}▛███▜${BODY}▌${RESET} `,
  `${BODY}▝▜█████▛▘${RESET}`,
  `  ${BODY}▘▘ ▝▝${RESET}  `,
  ` ${BODY}▗▟${EYES} ▖ ▘ ${BODY}▙▖${RESET}`,
  ` ${BODY}▐█▜██▛█▌ ▄ ▜ █▘ ▝▜ █▀ ▂${RESET}`,
  ` ${BODY}██▛▌ ▗ ▖ ▘ ▝${RESET}`,
]
const ALL = Array.from({ length: 32 }, (_, i) => String.fromCodePoint(0x2580 + i))
const GRID = [
  `\x1b[1mU+2580..259F${RESET}`,
  ALL.slice(0, 16).join(" "),
  ALL.slice(16).join(" "),
  `contiguous: ${BODY}${ALL.slice(0, 16).join("")}${RESET}`,
  `contiguous: ${BODY}${ALL.slice(16).join("")}${RESET}`,
  `tiles: ${BODY}${"▛▜".repeat(4)}\r\n       ${"▙▟".repeat(4)}${RESET}  ${"█".repeat(8)}`,
]
const FRAMES = [...GRID, "", ...MASCOT]

function Cell({ font, size, label }: { font: TerminalFontFamily; size: number; label: string }) {
  const host = useRef<HTMLDivElement>(null)
  useEffect(() => {
    const mode = document.documentElement.classList.contains("dark") ? "dark" : "light"
    const term = new Terminal({
      convertEol: true, cursorBlink: false, fontSize: size, fontFamily: terminalFontStack(font),
      theme: { ...buildXtermTheme(mode) }, minimumContrastRatio: xtermMinimumContrastRatio(mode),
      scrollback: 0, cols: 50, rows: FRAMES.length + 1,
    })
    term.open(host.current!)
    term.write(FRAMES.join("\r\n"))
    return () => term.dispose()
  }, [font, size])
  return <section style={{ margin: 8 }}>
    <h3 style={{ font: "12px sans-serif" }}>{label}</h3>
    <div ref={host} />
  </section>
}

function Page() {
  const [dark, setDark] = useState(true)
  useEffect(() => { document.documentElement.classList.toggle("dark", dark) }, [dark])
  const key = dark ? "dark" : "light"
  return <main style={{ padding: 12 }}>
    <h1>xterm block glyphs ({key})</h1>
    <button onClick={() => setDark(!dark)}>Toggle light/dark</button>
    <p>Every glyph must fill its cell: no gaps between adjacent cells, mascot body and eyes tile seamlessly.</p>
    <div style={{ display: "flex", flexWrap: "wrap" }}>
      {[12, 13, 16, 20].map((size) => <Cell key={`${key}-jb-${size}`} font="jetbrains" size={size} label={`JetBrains Mono ${size}px`} />)}
      {TERMINAL_FONTS.filter((f) => f.id !== "jetbrains").map((f) => <Cell key={`${key}-${f.id}`} font={f.id} size={14} label={`${f.name} 14px`} />)}
    </div>
  </main>
}
createRoot(document.getElementById("root")!).render(<Page />)

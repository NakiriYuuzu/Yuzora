/** Real DOM renderer regression. No native IPC, sessions, or external data. */
import { useEffect, useRef, useState } from "react"
import { createRoot } from "react-dom/client"
import { Terminal } from "@xterm/xterm"
import { FitAddon } from "@xterm/addon-fit"
import { Button } from "../src/components/ui/button"
import { terminalFontStack } from "../src/terminal/terminalFonts"
import "@xterm/xterm/css/xterm.css"
import "../src/styles.css"

const settle = () => new Promise<void>((resolve) => setTimeout(resolve, 120))
function screenData(cols: number, rows: number): string {
  return "\x1b[?2026h\x1b[0m\x1b[r\x1b[2J\x1b[H" + Array.from({ length: rows }, (_, row) => {
    const style = ["0", "1", "3", "1;3", "0"][row] ?? "0"
    const text = row === rows - 1 ? "BOTTOM_MARKER" : row === 4 ? "中文 ROW_5" : `ROW_${row + 1}`
    return `\x1b[${row + 1};1H\x1b[${style}m${text.padEnd(cols - (row === 4 ? 2 : 0), " ")}`
  }).join("") + "\x1b[0m\x1b[?2026l"
}
function glyphWidths(term: Terminal) {
  const screen = term.element!.querySelector(".xterm-screen")!.getBoundingClientRect()
  const rows = term.element!.querySelectorAll(".xterm-rows > div")
  return Array.from({ length: 5 }, (_, index) => {
    const node = rows[index]?.querySelector("span")?.firstChild
    const range = document.createRange()
    if (!node || !node.textContent?.length) throw new Error(`Row ${index + 1} did not render`)
    range.setStart(node, 0)
    range.setEnd(node, index === 4 ? 1 : 5)
    const actual = range.getBoundingClientRect().width
    const expected = screen.width / term.cols * (index === 4 ? 2 : 5)
    return { row: index + 1, actual, expected, pass: Math.abs(actual - expected) < 1 }
  })
}
function Probe() {
  const host = useRef<HTMLDivElement>(null)
  const runtime = useRef<{ term: Terminal; fit: FitAddon } | null>(null)
  const [running, setRunning] = useState(false)
  const [result, setResult] = useState("Ready. This regression requires a real browser; run on Windows WebView2.")
  useEffect(() => {
    const term = new Terminal({ fontFamily: terminalFontStack("jetbrains"), fontSize: 14, scrollback: 0, cursorBlink: false })
    const fit = new FitAddon()
    term.loadAddon(fit)
    term.open(host.current!)
    runtime.current = { term, fit }
    return () => { runtime.current = null; term.dispose() }
  }, [])
  async function run() {
    const current = runtime.current
    if (!current || running) return
    setRunning(true)
    const samples: object[] = []
    try {
      const { term, fit } = current
      await document.fonts.load(`14px ${terminalFontStack("jetbrains")}`)
      await settle()
      for (const size of [14, 26, 10, 30, 14, 18, 12, 24, 14]) {
        // Reproduce the production path: propose a grid, retain the old screen
        // while awaiting its peer, then commit font + grid with the full frame.
        const oldSize = term.options.fontSize
        term.options.fontSize = size
        const next = fit.proposeDimensions()
        term.options.fontSize = oldSize
        if (!next) throw new Error("Missing font dimensions")
        await settle()
        term.options.fontSize = size
        term.resize(next.cols, next.rows)
        await new Promise<void>((resolve) => term.write(screenData(next.cols, next.rows), resolve))
        await settle()
        const widths = glyphWidths(term)
        const bottom = term.buffer.active.getLine(term.rows - 1)?.translateToString(true).trim()
        const pass = widths.every((width) => width.pass) && bottom === "BOTTOM_MARKER"
        samples.push({ size, pass, widths, bottom })
        if (!pass) throw new Error(`Font ${size}px failed actual glyph layout`)
      }
      setResult(JSON.stringify({ pass: true, samples }, null, 2))
    } catch (error) { setResult(JSON.stringify({ pass: false, error: String(error), samples }, null, 2)) }
    finally { setRunning(false) }
  }
  return <main style={{ padding: 12 }}>
    <h1>xterm font width regression</h1>
    <Button disabled={running} onClick={() => void run()}>Run font regression</Button>
    <div ref={host} style={{ width: 600, height: 450, marginTop: 12 }} />
    <pre aria-label="Regression result" style={{ whiteSpace: "pre-wrap" }}>{result}</pre>
  </main>
}
createRoot(document.getElementById("root")!).render(<Probe />)

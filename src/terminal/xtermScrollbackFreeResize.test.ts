import { Terminal } from "@xterm/xterm"
import { afterEach, describe, expect, it } from "vitest"

import { TerminalOutputQueue } from "./terminalOutputQueue"

// HerdrTerminalPage's buffer options. HerdrNativeDialog and MachineInteractiveDialog
// also use `scrollback: 0`.
const herdrOptions = { convertEol: true, scrollback: 0, scrollOnUserInput: false, smoothScrollDuration: 0 }

// HERDR full frames (connector and official client) repaint every cell of every row,
// so each row is "full" for xterm's reflow, including the blank rows below the output.
// `outputLines` rows of text, cursor on the next row.
function herdrFrame(cols: number, rows: number, outputLines: number) {
  let frame = "\x1b[?2026h\x1b[?25l\x1b[2J"
  for (let row = 1; row <= rows; row++) {
    const text = row <= outputLines ? `TICK_${String(row).padStart(3, "0")} ${"-".repeat(69)}` : ""
    frame += `\x1b[${row};1H\x1b[0;39;49m${text.padEnd(cols, " ").slice(0, cols)}`
  }
  return `${frame}\x1b[?2026l\x1b[${Math.min(outputLines + 1, rows)};1H\x1b[?25h`
}

const sleep = (ms: number) => new Promise((resolve) => window.setTimeout(resolve, ms))
const write = (term: Terminal, data: string) => new Promise<void>((resolve) => term.write(data, resolve))

describe("patched xterm: resizing a scrollback-free buffer (#183)", () => {
  let term: Terminal | undefined
  afterEach(() => {
    term?.dispose()
    term = undefined
  })

  it("keeps the viewport inside the buffer when a painted screen shrinks while the cursor is high", async () => {
    term = new Terminal({ cols: 80, rows: 24, ...herdrOptions })
    await write(term, herdrFrame(80, 24, 7))

    term.resize(60, 20)
    expect(term.buffer.active.baseY).toBe(0)
    expect(term.buffer.active.length).toBe(20)
  })

  it("still reaches the write callback after HERDR full frames shrink and grow the terminal", async () => {
    term = new Terminal({ cols: 80, rows: 24, ...herdrOptions })
    const xterm = term
    let processed = 0
    // Wired like HerdrTerminalPage: the queue waits for xterm's write callback before
    // its next flush, and a full frame resizes xterm in `beforeWrite`.
    const queue = new TerminalOutputQueue((data, onProcessed) => {
      xterm.write(data, () => {
        processed += 1
        onProcessed()
      })
    }, true)
    const settled = async (count: number) => {
      for (let waited = 0; processed < count && waited < 1000; waited += 10) await sleep(10)
      return processed >= count
    }
    const showFullFrame = (cols: number, rows: number, outputLines: number) => {
      queue.replace(herdrFrame(cols, rows, outputLines), () => {
        if (xterm.cols !== cols || xterm.rows !== rows) xterm.resize(cols, rows)
      })
      queue.flushNow()
    }

    try {
      // Shrink while the cursor is still high (row 8)...
      showFullFrame(80, 24, 7)
      expect(await settled(1)).toBe(true)
      // ...then output fills the smaller screen, leaving the cursor on the last row...
      showFullFrame(60, 20, 19)
      expect(await settled(2)).toBe(true)
      // ...then grow. Writing row 30 used to throw inside xterm's write timer, so the
      // callback never ran and the queue stayed `writing` forever: a frozen terminal.
      showFullFrame(100, 30, 29)
      expect(await settled(3)).toBe(true)
      const row = (index: number) => xterm.buffer.active.getLine(xterm.buffer.active.viewportY + index)?.translateToString().trimEnd()
      expect(row(28)).toMatch(/^TICK_029 -+$/)

      queue.push("\x1b[30;1Hstill alive")
      expect(await settled(4)).toBe(true)
      expect(row(29)).toBe("still alive")
    } finally {
      queue.dispose()
    }
  })

  it("still moves wrapped output into scrollback when the buffer keeps history", async () => {
    term = new Terminal({ cols: 80, rows: 24, scrollback: 100 })
    await write(term, herdrFrame(80, 24, 24))

    term.resize(60, 24)
    expect(term.buffer.active.baseY).toBeGreaterThan(0)
    expect(term.buffer.active.length).toBe(term.buffer.active.baseY + 24)
  })
})

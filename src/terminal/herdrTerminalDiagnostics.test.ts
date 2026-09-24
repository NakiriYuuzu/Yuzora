import { afterEach, beforeEach, expect, it, vi } from "vitest"
import { clearMocks, mockIPC } from "@tauri-apps/api/mocks"

import {
  HERDR_TERMINAL_DIAGNOSTICS_EVENT,
  describeTerminalBytes,
  flushHerdrTerminalDiagnostics,
  keyCombo,
  observeHerdrTerminalKeys,
  recordHerdrTerminalMetric,
  recordHerdrTerminalOutput,
  setHerdrTerminalDiagnosticsEnabled,
  summarize,
  timeHerdrTerminalIpc,
} from "./herdrTerminalDiagnostics"

interface CapturedEvent { level: string; kind: string; source: string; event: string; metadata: Record<string, unknown> }
let captured: CapturedEvent[] = []
let commands: string[] = []

beforeEach(() => {
  captured = []
  commands = []
  mockIPC((cmd, payload) => {
    commands.push(cmd)
    if (cmd === "log_event") captured.push((payload as { event: CapturedEvent }).event)
  })
})
afterEach(() => {
  setHerdrTerminalDiagnosticsEnabled(false)
  clearMocks()
  vi.useRealTimers()
})

it("does nothing and never invokes IPC while disabled", async () => {
  setHerdrTerminalDiagnosticsEnabled(false)
  recordHerdrTerminalMetric({ kind: "frame", full: true, bytes: 100 })
  expect(flushHerdrTerminalDiagnostics()).toBeNull()
  await expect(timeHerdrTerminalIpc("pane.get", async () => 7)).resolves.toBe(7)
  await Promise.resolve()
  expect(commands).toEqual([])
})

it("summarizes one window into a debug-level log record", async () => {
  setHerdrTerminalDiagnosticsEnabled(true)
  recordHerdrTerminalMetric({ kind: "wheel", strategy: "terminal", rows: 6 }, 0)
  recordHerdrTerminalMetric({ kind: "frame", full: true, bytes: 1000 }, 10)
  recordHerdrTerminalMetric({ kind: "frame", full: false, bytes: 200 }, 30)
  recordHerdrTerminalMetric({ kind: "write", ms: 4, bytes: 1000 }, 31)
  recordHerdrTerminalMetric({ kind: "ipc", command: "terminal.scroll", ms: 3, ok: true }, 32)
  recordHerdrTerminalMetric({ kind: "ipc", command: "terminal.scroll", ms: 9, ok: false }, 33)
  const summary = flushHerdrTerminalDiagnostics(5000)!
  expect(summary.windowMs).toBe(5000)
  expect(summary.wheel).toMatchObject({ events: 1, strategy: { terminal: 1 } })
  expect(summary.frames).toMatchObject({ n: 2, full: 1, delta: 1, gapMs: { n: 1, p50: 20 } })
  expect(summary.ipc["terminal.scroll"]).toMatchObject({ n: 2, failed: 1, max: 9 })
  await vi.waitFor(() => expect(captured).toHaveLength(1))
  expect(captured[0]).toMatchObject({ level: "debug", kind: "debug", source: "herdr-terminal", event: HERDR_TERMINAL_DIAGNOSTICS_EVENT })
  expect(flushHerdrTerminalDiagnostics()).toBeNull()
})

it("flushes automatically after the window elapses", async () => {
  vi.useFakeTimers()
  setHerdrTerminalDiagnosticsEnabled(true)
  recordHerdrTerminalMetric({ kind: "wheel", strategy: "pane", rows: 1 })
  vi.advanceTimersByTime(5000)
  vi.useRealTimers()
  await vi.waitFor(() => expect(captured).toHaveLength(1))
})

it("computes nearest-rank percentiles", () => {
  expect(summarize([5, 1, 3, 2, 4])).toEqual({ n: 5, p50: 3, p95: 5, max: 5 })
  expect(summarize([])).toEqual({ n: 0, p50: 0, p95: 0, max: 0 })
})

it("describes control sequences without leaking typed text", () => {
  expect(describeTerminalBytes("\x1bq")).toBe("ESC 71")
  expect(describeTerminalBytes("\x1b")).toBe("ESC")
  expect(describeTerminalBytes("\x0b")).toBe("0b")
  expect(describeTerminalBytes("hello")).toBe("text")
  expect(describeTerminalBytes("\x1b[1;2;3;4;5;6_")).toBe("text")
  expect(describeTerminalBytes("")).toBe("none")
})

it("names chords by physical key", () => {
  expect(keyCombo({ altKey: true, ctrlKey: false, metaKey: false, shiftKey: false, key: "œ", code: "KeyQ" })).toBe("alt+q")
  expect(keyCombo({ altKey: false, ctrlKey: true, metaKey: false, shiftKey: true, key: "!", code: "Digit1" })).toBe("ctrl+shift+1")
  expect(keyCombo({ altKey: false, ctrlKey: false, metaKey: false, shiftKey: false, key: "Escape", code: "Escape" })).toBe("esc")
})

it("records modified keys with the bytes the terminal emitted, and ignores plain typing", async () => {
  setHerdrTerminalDiagnosticsEnabled(true)
  const container = document.createElement("div")
  const input = container.appendChild(document.createElement("textarea"))
  document.body.appendChild(container)
  const observer = observeHerdrTerminalKeys(container)
  input.dispatchEvent(new KeyboardEvent("keydown", { key: "a", code: "KeyA", bubbles: true }))
  observer.noteData("a")
  input.dispatchEvent(new KeyboardEvent("keydown", { key: "q", code: "KeyQ", altKey: true, bubbles: true }))
  observer.noteData("\x1bq")
  await new Promise((resolve) => setTimeout(resolve, 0))
  const summary = flushHerdrTerminalDiagnostics()!
  expect(summary.keys).toEqual([{ combo: "alt+q", code: "KeyQ", altGraph: false, prevented: false, emitted: "ESC 71" }])
  observer.dispose()
  container.remove()
})

it("measures frame gaps within one window, not across an idle period between windows", () => {
  setHerdrTerminalDiagnosticsEnabled(true)
  recordHerdrTerminalMetric({ kind: "frame", full: false, bytes: 10 }, 0)
  recordHerdrTerminalMetric({ kind: "frame", full: false, bytes: 10 }, 20)
  flushHerdrTerminalDiagnostics(5000)
  // The terminal was idle for a minute before the next window starts.
  recordHerdrTerminalMetric({ kind: "frame", full: false, bytes: 10 }, 65_000)
  recordHerdrTerminalMetric({ kind: "frame", full: false, bytes: 10 }, 65_030)
  const metadata = flushHerdrTerminalDiagnostics(70_000)
  expect(metadata?.frames.gapMs).toMatchObject({ n: 1, max: 30 })
})

it("records terminal output sizes as UTF-8 bytes, not UTF-16 code units", () => {
  setHerdrTerminalDiagnosticsEnabled(true)
  recordHerdrTerminalOutput("frame", "中文🙂", { full: false }, 0)
  recordHerdrTerminalOutput("write", "中文🙂", { ms: 1 }, 1)
  const metadata = flushHerdrTerminalDiagnostics(5)
  // 2 CJK chars × 3 bytes + 1 emoji × 4 bytes; `.length` would report 4.
  expect(metadata?.frames.bytes).toMatchObject({ n: 1, max: 10 })
  expect(metadata?.writes.bytes).toMatchObject({ n: 1, max: 10 })
})

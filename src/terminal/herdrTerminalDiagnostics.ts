import { invoke } from "@/lib/ipc"
import { getLogLevel } from "@/features/logs/logQuery"

/**
 * Opt-in, content-free Herdr terminal diagnostics. Enabled only while the app
 * log level is `debug` (or the VITE scroll-diagnostics build flag is set).
 * Records counts, sizes and durations; never terminal output or typed text.
 */
/** `terminal.scroll.frame`: connector scroll sent → next frame received (ok=false: timed out). */
export type HerdrTerminalIpcCommand = "terminal.scroll" | "terminal.scroll.frame" | "pane.get" | "pane.scroll"
export type HerdrTerminalMetric =
  | { kind: "wheel"; strategy: "terminal" | "pane"; rows: number }
  | { kind: "ipc"; command: HerdrTerminalIpcCommand; ms: number; ok: boolean }
  | { kind: "frame"; full: boolean; bytes: number }
  | { kind: "write"; ms: number; bytes: number }
  | { kind: "key"; combo: string; code: string; altGraph: boolean; prevented: boolean; emitted: string }

export const HERDR_TERMINAL_DIAGNOSTICS_EVENT = "herdr.terminal.diagnostics"
export const HERDR_TERMINAL_DIAGNOSTICS_WINDOW_MS = 5000
const MAX_SAMPLES = 2000
const MAX_KEYS = 32

type Summary = { n: number; p50: number; p95: number; max: number }
interface Window {
  startedAt: number
  wheel: { events: number; rows: number[]; strategy: Record<string, number> }
  ipc: Record<string, { ms: number[]; failed: number }>
  frames: { full: number; delta: number; bytes: number[]; gaps: number[]; lastAt: number | null }
  writes: { ms: number[]; bytes: number[] }
  keys: Array<Omit<Extract<HerdrTerminalMetric, { kind: "key" }>, "kind">>
}

let enabled = import.meta.env.VITE_YUZORA_SCROLL_DIAGNOSTICS === "1"
let current: Window | null = null
let timer: ReturnType<typeof setTimeout> | null = null
let lastFrameAt: number | null = null

function freshWindow(now: number): Window {
  return {
    startedAt: now,
    wheel: { events: 0, rows: [], strategy: {} },
    ipc: {},
    frames: { full: 0, delta: 0, bytes: [], gaps: [], lastAt: null },
    writes: { ms: [], bytes: [] },
    keys: [],
  }
}
function push(samples: number[], value: number) {
  if (samples.length < MAX_SAMPLES && Number.isFinite(value)) samples.push(value)
}
export function summarize(samples: readonly number[]): Summary {
  const sorted = [...samples].sort((a, b) => a - b)
  const at = (q: number) => sorted.length ? sorted[Math.min(sorted.length - 1, Math.floor(q * sorted.length))] : 0
  const round = (value: number) => Math.round(value * 100) / 100
  return { n: sorted.length, p50: round(at(0.5)), p95: round(at(0.95)), max: round(sorted.at(-1) ?? 0) }
}

export function isHerdrTerminalDiagnosticsEnabled() { return enabled }
export function setHerdrTerminalDiagnosticsEnabled(next: boolean) {
  if (enabled === next) return
  enabled = next
  if (!next) {
    if (timer) clearTimeout(timer)
    timer = null
    current = null
    lastFrameAt = null
  }
}

/** Startup sync with the persisted app log level (Settings → Logs → debug). */
export function initHerdrTerminalDiagnosticsFromLogLevel(): Promise<void> {
  return getLogLevel()
    .then((level) => { if (level === "debug") setHerdrTerminalDiagnosticsEnabled(true) })
    .catch(() => undefined)
}

export function recordHerdrTerminalMetric(metric: HerdrTerminalMetric, now = performance.now()) {
  if (!enabled) return
  const window = current ??= freshWindow(now)
  timer ??= setTimeout(() => { timer = null; flushHerdrTerminalDiagnostics() }, HERDR_TERMINAL_DIAGNOSTICS_WINDOW_MS)
  switch (metric.kind) {
    case "wheel":
      window.wheel.events++
      push(window.wheel.rows, metric.rows)
      window.wheel.strategy[metric.strategy] = (window.wheel.strategy[metric.strategy] ?? 0) + 1
      break
    case "ipc": {
      const bucket = window.ipc[metric.command] ??= { ms: [], failed: 0 }
      push(bucket.ms, metric.ms)
      if (!metric.ok) bucket.failed++
      break
    }
    case "frame":
      if (metric.full) window.frames.full++
      else window.frames.delta++
      push(window.frames.bytes, metric.bytes)
      if (lastFrameAt !== null) push(window.frames.gaps, now - lastFrameAt)
      lastFrameAt = now
      break
    case "write":
      push(window.writes.ms, metric.ms)
      push(window.writes.bytes, metric.bytes)
      break
    case "key": {
      const { kind: _kind, ...key } = metric
      if (window.keys.length < MAX_KEYS) window.keys.push(key)
      break
    }
  }
}

/** Writes one summary record for the current window. Returns it for tests. */
export function flushHerdrTerminalDiagnostics(now = performance.now()) {
  const window = current
  current = null
  if (timer) { clearTimeout(timer); timer = null }
  if (!enabled || !window) return null
  const frames = window.frames.full + window.frames.delta
  const metadata = {
    windowMs: Math.round(now - window.startedAt),
    wheel: { events: window.wheel.events, rows: summarize(window.wheel.rows), strategy: window.wheel.strategy },
    ipc: Object.fromEntries(Object.entries(window.ipc).map(([command, bucket]) => [command, { ...summarize(bucket.ms), failed: bucket.failed }])),
    frames: { n: frames, full: window.frames.full, delta: window.frames.delta, bytes: summarize(window.frames.bytes), gapMs: summarize(window.frames.gaps) },
    writes: { ms: summarize(window.writes.ms), bytes: summarize(window.writes.bytes) },
    keys: window.keys,
  }
  void invoke("log_event", {
    event: {
      level: "debug",
      kind: "debug",
      source: "herdr-terminal",
      workspace_path: null,
      event: HERDR_TERMINAL_DIAGNOSTICS_EVENT,
      message: `herdr terminal diagnostics ${metadata.windowMs}ms`,
      metadata,
    },
  }).catch(() => undefined)
  return metadata
}

/** Measures one IPC promise without changing its result. */
export function timeHerdrTerminalIpc<T>(command: HerdrTerminalIpcCommand, run: () => Promise<T>): Promise<T> {
  if (!enabled) return run()
  const startedAt = performance.now()
  return run().then(
    (value) => { recordHerdrTerminalMetric({ kind: "ipc", command, ms: performance.now() - startedAt, ok: true }); return value },
    (error: unknown) => { recordHerdrTerminalMetric({ kind: "ipc", command, ms: performance.now() - startedAt, ok: false }); throw error },
  )
}

/**
 * Content-free description of bytes emitted for a modified key. Control
 * sequences up to 8 bytes are described (`ESC 71`); anything else is `text`.
 */
export function describeTerminalBytes(data: string): string {
  if (!data) return "none"
  const first = data.charCodeAt(0)
  if ((first >= 0x20 && first !== 0x7f) || data.length > 8) return "text"
  return [...new TextEncoder().encode(data)]
    .map(byte => byte === 0x1b ? "ESC" : byte.toString(16).padStart(2, "0"))
    .join(" ")
}

export function keyCombo(event: Pick<KeyboardEvent, "altKey" | "ctrlKey" | "metaKey" | "shiftKey" | "key" | "code">): string {
  const key = event.key === "Escape" ? "esc"
    : /^Key[A-Z]$/.test(event.code) ? event.code.slice(3).toLowerCase()
      : /^Digit\d$/.test(event.code) ? event.code.slice(5)
        : event.key.toLowerCase()
  return [event.ctrlKey && "ctrl", event.altKey && "alt", event.shiftKey && "shift", event.metaKey && "meta", key]
    .filter(Boolean).join("+")
}

/**
 * Observes modified keydowns inside one terminal without replacing xterm's
 * single custom key handler. Call `noteData` from the terminal's onData path.
 */
export function observeHerdrTerminalKeys(container: HTMLElement) {
  let pending: { event: KeyboardEvent; data: string } | null = null
  const onKeyDown = (event: KeyboardEvent) => {
    if (!enabled) return
    if (!(event.altKey || event.ctrlKey || event.metaKey || event.key === "Escape")) return
    if (["Alt", "Control", "Meta", "Shift"].includes(event.key)) return
    const entry = { event, data: "" }
    pending = entry
    setTimeout(() => {
      if (pending === entry) pending = null
      recordHerdrTerminalMetric({
        kind: "key",
        combo: keyCombo(event),
        code: event.code,
        altGraph: event.getModifierState?.("AltGraph") ?? false,
        prevented: event.defaultPrevented,
        emitted: describeTerminalBytes(entry.data),
      })
    }, 0)
  }
  container.addEventListener("keydown", onKeyDown, true)
  return {
    noteData(data: string) { if (pending) pending.data += data },
    dispose() { container.removeEventListener("keydown", onKeyDown, true); pending = null },
  }
}

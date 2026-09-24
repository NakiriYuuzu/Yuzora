/** Real xterm + production HERDR page; only the IPC peer is controlled. */
import { createRoot } from 'react-dom/client'
import { useState } from 'react'
import { Button } from '../src/components/ui/button'
import { Terminal } from '@xterm/xterm'
import type { Channel } from '@tauri-apps/api/core'
import '@xterm/xterm/css/xterm.css'
import '../src/styles.css'
import '../src/lib/i18n'
import { installDemoRuntime, capabilities, sessions, snapshot } from '../src/demo/runtime'
import { normalizeHerdrSnapshot } from '../src/lib/herdrNormalize'
import { useHerdrStore } from '../src/state/herdrStore'
import { useTerminalSettingsStore } from '../src/state/terminalSettingsStore'
import type { HerdrCapabilities, HerdrTerminalEvent } from '../src/lib/herdrTypes'

installDemoRuntime()
const normalized = normalizeHerdrSnapshot(snapshot, 'studio')
const caps = { ...capabilities, api: { ...capabilities.api, methods: capabilities.api.methods.filter(method => !['pane.get', 'pane.scroll'].includes(method)) } } as HerdrCapabilities
useHerdrStore.setState({ sessions, selectedSessionName: 'studio', selectedSpaceId: 'studio', capabilities: caps, snapshot: normalized, runtimesBySession: { studio: { connectionState: 'ready', capabilities: caps, snapshot: normalized, errorMessage: null, worktreeInventory: null } } })
useTerminalSettingsStore.setState({ fontSize: 14, fontFamily: 'menlo' })
let terminal: Terminal | undefined
let channel: Channel<HerdrTerminalEvent> | undefined
let seq = 0
const trace: { event: string; cols: number; rows: number; at: number }[] = []
const record = (event: string, cols: number, rows: number) => trace.push({ event, cols, rows, at: performance.now() })
const open = Terminal.prototype.open
Terminal.prototype.open = function(parent) { terminal = this; return open.call(this, parent) }
const resize = Terminal.prototype.resize
Terminal.prototype.resize = function(cols, rows) { record('xterm.resize', cols, rows); return resize.call(this, cols, rows) }
const internals = (window as unknown as { __TAURI_INTERNALS__: { invoke: (cmd: string, args: Record<string, unknown>) => Promise<unknown> } }).__TAURI_INTERNALS__
const invoke = internals.invoke
internals.invoke = async (command, args) => {
  if (command === 'herdr_terminal_open') {
    channel = args.onEvent as Channel<HerdrTerminalEvent>
    record('open', Number(args.cols), Number(args.rows))
    return { sessionId: 'render-probe', target: args.target, mode: 'control', role: 'controller', takeover: true, cols: args.cols, rows: args.rows }
  }
  if (command === 'herdr_terminal_resize') { record('remote.resize', Number(args.cols), Number(args.rows)); return null }
  if (command === 'herdr_terminal_input' || command === 'herdr_terminal_release') return null
  return invoke(command, args)
}
const { HerdrTerminalPage } = await import('../src/app/panels/HerdrTerminalPage')
createRoot(document.getElementById('root')!).render(<div id="surface" style={{ width: 900, height: 600, display: 'flex' }}><HerdrTerminalPage herdrSessionId="studio" terminalId="term-build" paneId="pane-build" herdrTabId="build" active visible /></div>)
const delay = () => new Promise(resolve => setTimeout(resolve, 120))
async function waitUntil(ready: () => boolean, message: string) {
  const deadline = performance.now() + 3000
  while (!ready()) {
    if (performance.now() >= deadline) throw new Error(message)
    await delay()
  }
}
const info = () => ({ cols: terminal?.cols, rows: terminal?.rows, cursorY: terminal?.buffer.active.cursorY, viewportY: terminal?.buffer.active.viewportY, baseY: terminal?.buffer.active.baseY, lines: Array.from({ length: terminal?.rows ?? 0 }, (_, row) => terminal?.buffer.active.getLine(row)?.translateToString(true) ?? ''), trace: [...trace] })
async function frame(cols: number, rows: number, data?: string) {
  if (!terminal || !channel) throw new Error('Terminal not ready')
  const ansi = data ?? '\x1b[?2026h\x1b[0m\x1b[r\x1b[2J\x1b[H' + Array.from({ length: rows }, (_, row) => `\x1b[${row + 1};1H${row === rows - 1 ? 'BOTTOM_MARKER' : `ROW_${row + 1}`}`).join('') + '\x1b[?2026l'
  channel.onmessage({ type: 'frame', sessionId: 'render-probe', seq: ++seq, full: true, encoding: 'ansi', width: cols, height: rows, bytesBase64: btoa(ansi) })
  await delay()
  return info()
}
Object.assign(window, { herdrRenderProbe: {
  info, frame,
  async replay(frames: { width: number; height: number; full: boolean; bytes: string }[]) {
    for (const value of frames) {
      if (!channel) throw new Error('Terminal not ready')
      record('frame', value.width, value.height)
      channel.onmessage({ type: 'frame', sessionId: 'render-probe', seq: ++seq, full: value.full, encoding: 'ansi', width: value.width, height: value.height, bytesBase64: value.bytes })
      await delay()
    }
    return info()
  },
  async font(size: number) { useTerminalSettingsStore.setState({ fontSize: size }); await delay(); return info() },
  async container(width: number, height: number) { Object.assign(document.getElementById('surface')!.style, { width: `${width}px`, height: `${height}px` }); await delay(); return info() },
} })

// Manual controls keep the regression runnable in the Codex in-app browser.
function ReplayControls() {
  const [result, setResult] = useState('Ready. Capture frames with YUZORA_HERDR_FRAME_TRACE first.')
  const [running, setRunning] = useState(false)
  async function run(version: '090' | '091') {
    setRunning(true)
    try {
      const response = await fetch(`/output/herdr-render/frames-${version}.json`)
      if (!response.ok) throw new Error(`Frame capture HTTP ${response.status}`)
      const frames = (await response.json() as { stream: string; width: number; height: number; full: boolean; bytes: string }[])
        .filter(value => value.stream === 'controller')
      if (!terminal || !channel || !frames.length) throw new Error('Terminal or frames not ready')
      const baselineStart = trace.length
      const changedBaseline = terminal.options.fontSize !== 14
      useTerminalSettingsStore.setState({ fontSize: 14 })
      await waitUntil(() => terminal?.options.fontSize === 14
        && (!changedBaseline || trace.slice(baselineStart).some(value => value.event === 'remote.resize')),
      'Baseline font change did not publish resize')
      trace.length = 0
      await frame(terminal.cols, terminal.rows)
      useTerminalSettingsStore.setState({ fontSize: 22 })
      await waitUntil(() => terminal?.options.fontSize === 22
        && trace.some(value => value.event === 'remote.resize'), 'Font change did not publish resize')
      for (const value of frames) {
        record('frame', value.width, value.height)
        channel.onmessage({ type: 'frame', sessionId: 'render-probe', seq: ++seq, full: value.full, encoding: 'ansi', width: value.width, height: value.height, bytesBase64: value.bytes })
        await delay()
      }
      const final = info()
      const expected = Array.from({ length: 29 }, (_, row) => `SCROLL_ROW_${372 + row}`)
      if (!expected.every((value, row) => final.lines[row]?.trim() === value)
        || final.lines[29]?.trim() !== 'sh-3.2$') throw new Error(`Frame contents displaced: ${JSON.stringify(final.lines)}`)
      const unmatchedResize = trace.filter((value, index) => value.event === 'xterm.resize'
        && !(trace[index - 1]?.event === 'frame' && trace[index - 1]?.cols === value.cols && trace[index - 1]?.rows === value.rows))
      if (unmatchedResize.length) throw new Error('Viewport reflowed before its authoritative frame')
      setResult(JSON.stringify({ pass: true, version, frames: frames.length, cols: final.cols, rows: final.rows, cursorY: final.cursorY, viewportY: final.viewportY, baseY: final.baseY, first: final.lines[0]?.trim(), last: final.lines[28]?.trim(), resizeEvents: trace }, null, 2))
    } catch (error) { setResult(JSON.stringify({ error: String(error), fontSize: terminal?.options.fontSize, state: info() }, null, 2)) }
    finally { setRunning(false) }
  }
  return <section style={{ position: 'absolute', left: 920, top: 12, width: 420 }}>
    <Button disabled={running} onClick={() => void run('090')}>Replay HERDR 0.9.0</Button>
    <Button disabled={running} onClick={() => void run('091')}>Replay HERDR 0.9.1</Button>
    <pre aria-label="Regression result" style={{ whiteSpace: 'pre-wrap', fontSize: 12 }}>{result}</pre>
  </section>
}
const controls = document.createElement('div')
document.body.append(controls)
createRoot(controls).render(<ReplayControls />)

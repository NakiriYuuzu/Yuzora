/** Transport seam for Herdr terminal pages. */

import {
  herdrTerminalInput,
  herdrTerminalMouse,
  herdrTerminalOpen,
  herdrTerminalRelease,
  herdrTerminalResize,
  herdrTerminalScroll
} from "@/lib/herdrIpc"
import { herdrErrorKind } from "@/lib/herdrErrors"
import { readPaneScroll, setPaneScroll } from "./herdrScrollIpc"
import { recordHerdrTerminalMetric, timeHerdrTerminalIpc } from "./herdrTerminalDiagnostics"
import type { PaneScrollController, PaneScrollInfo } from "./herdrScrollController"
import type {
  HerdrMouseAction,
  HerdrTerminalEvent,
  HerdrTerminalMode,
  HerdrTerminalRole
} from "@/lib/herdrTypes"

export type TerminalTransportOutputEvent = {
  type: "output"
  data: string
  seq: number
  droppedBytes: number
  truncated: boolean
  full?: boolean
  width?: number
  height?: number
}

export type TerminalTransportEvent =
  | TerminalTransportOutputEvent
  /** `reason` is Herdr's `terminal.closed` reason, e.g. `terminal attach taken over`. */
  | { type: "exit"; code: number | null; reason?: string | null }
  /** `code` is `host-stream-closed` when a remote connector stream died; the pane is still alive. */
  | { type: "error"; message: string; code?: string }
  | { type: "resync"; message: string }
  | { type: "control"; mode: HerdrTerminalMode; role: HerdrTerminalRole }

export function normalizeTerminalWheelRows(
  deltaY: number,
  deltaMode: number,
  viewportRows: number
): number {
  if (!Number.isFinite(deltaY) || deltaY === 0) return 0
  const rows = Math.max(1, Math.floor(viewportRows))
  const rawRows = deltaMode === 2
    ? Math.abs(deltaY) * rows
    : deltaMode === 1
      ? Math.abs(deltaY)
      : Math.abs(deltaY) / 16
  return Math.min(rows, Math.max(1, Math.round(rawRows)))
}

/** Zero-based terminal cell under the pointer, as HERDR's connector expects. */
export interface TerminalCell {
  column: number
  row: number
}

export function terminalWheelCell(
  screen: { left: number; top: number; width: number; height: number },
  cols: number,
  rows: number,
  clientX: number,
  clientY: number
): TerminalCell | undefined {
  if (!(screen.width > 0 && screen.height > 0 && cols >= 1 && rows >= 1)) return undefined
  if (!Number.isFinite(clientX) || !Number.isFinite(clientY)) return undefined
  const cell = (offset: number, size: number, count: number) =>
    Math.min(count - 1, Math.max(0, Math.floor(offset / (size / count))))
  return {
    column: cell(clientX - screen.left, screen.width, Math.floor(cols)),
    row: cell(clientY - screen.top, screen.height, Math.floor(rows))
  }
}

// HERDR pane.get is a remote IPC/RPC round trip. Keep a short-lived local
// snapshot for standalone callers. Mounted terminal pages share their pane
// controller with the scrollbar, so wheel/drag never use separate cached positions.
const PANE_SCROLL_CACHE_MS = 160
/**
 * A connector `terminal.scroll` is acknowledged as soon as the command is
 * queued (p50 1 ms); the viewport only moves when Herdr sends the redrawn
 * frame. Waiting for that frame (bounded, for edges where nothing redraws)
 * lets wheel bursts coalesce instead of queuing a backlog that keeps
 * replaying after the wheel stops.
 */
const SCROLL_FRAME_WAIT_MS = 100
/**
 * HERDR turns each connector `terminal.scroll` routed to an application into
 * exactly one mouse report or alternate-scroll key, ignoring `lines` (xterm.js
 * likewise reports once per wheel event). Keep one command per queued wheel
 * event, sent one at a time so the shared stream queue stays free for input,
 * but cap a frame window at what a 60 Hz wheel yields within
 * SCROLL_FRAME_WAIT_MS so a slow remote cannot replay a backlog afterwards.
 */
const APPLICATION_WHEEL_REPORTS_PER_FRAME = 6

export interface TerminalTransportOpenArgs {
  cols: number
  rows: number
  onEvent: (event: TerminalTransportEvent) => void
}

export interface TerminalTransport {
  open(args: TerminalTransportOpenArgs): Promise<void>
  write(data: string): Promise<void>
  paste(text: string): Promise<void>
  resize(cols: number, rows: number): Promise<void>
  scroll?(delta: number, cell?: TerminalCell): Promise<void>
  /** Best-effort left-button event; resolves once the queued events are sent. */
  mouse?(action: HerdrMouseAction, cell: TerminalCell, modifiers: number): Promise<void>
  release(): Promise<void>
  /**
   * Clear the active connector without sending a backend release. Component
   * teardown must release through the attachment registry exactly once.
   */
  detach?(): void
  /**
   * Clear the active connector generation without backend release so a caller
   * can transfer release ownership to the attachment registry before reopening.
   */
  detachSession?(): string | null
  /**
   * Permanent teardown. Later open/takeControl/resync reopen paths must no-op
   * and any late open result must be released without re-registering attachments.
   */
  dispose?(): Promise<void>
  /** True when input should be forwarded (control mode). Observer stays read-only. */
  canWrite(): boolean
  getControlMode?(): HerdrTerminalMode
  getRole?(): HerdrTerminalRole
  getSessionId?(): string | null
  isDisposed?(): boolean
  /** Reopen the same target in control mode with takeover after explicit user action. */
  takeControl?(): Promise<void>
}

const frameDecoder = typeof TextDecoder !== "undefined"
  ? new TextDecoder("utf-8", { fatal: false })
  : null
const inputEncoder = new TextEncoder()

function decodeFrameBytes(bytesBase64: string): string {
  try {
    // Older WKWebView/WebView2 versions still need the atob path.
    const byteArray = Uint8Array as Uint8ArrayConstructor & {
      fromBase64?: (value: string) => Uint8Array
    }
    let bytes: Uint8Array
    if (typeof byteArray.fromBase64 === "function") {
      bytes = byteArray.fromBase64(bytesBase64)
    } else if (typeof atob === "function") {
      const binary = atob(bytesBase64)
      bytes = new Uint8Array(binary.length)
      for (let i = 0; i < binary.length; i++) bytes[i] = binary.charCodeAt(i)
    } else {
      return bytesBase64
    }
    if (frameDecoder) return frameDecoder.decode(bytes)
    let out = ""
    for (let i = 0; i < bytes.length; i++) out += String.fromCharCode(bytes[i])
    return out
  } catch {
    return bytesBase64
  }
}

export interface HerdrTerminalTransportOptions {
  /** Live terminal identity used as connector `target`. */
  terminalId: string
  paneId?: string | null
  mode?: HerdrTerminalMode
  /** Default true for visible terminal ownership (control + takeover). */
  takeover?: boolean
  /** Named Herdr session for HERDR_SESSION connector routing. */
  sessionName?: string | null
  /** True when the official pane scroll API is available for this terminal. */
  paneScrollEnabled?: () => boolean
  /** True when at least one verified scroll transport is available. */
  scrollEnabled?: () => boolean
  /** Disable the legacy terminal command when pane scrolling is the only safe transport. */
  terminalScrollEnabled?: () => boolean
  /**
   * Send physical wheels through the connector command so HERDR routes them
   * by the child's modes (mouse report, alternate scroll or host scrollback),
   * even when pane scrolling is otherwise the preferred or only transport.
   */
  applicationWheelEnabled?: () => boolean
  /** True when the connector accepts `terminal.mouse` (HERDR 0.9.2+). */
  mouseEnabled?: () => boolean
  /** Publish the authoritative pane state returned by pane.scroll without another read. */
  onPaneScroll?: (state: PaneScrollInfo) => void
  /** Share the scrollbar's optimistic position and immediate writer. */
  paneScrollController?: () => PaneScrollController | null
  onAttachment?: (info: {
    sessionId: string
    mode: HerdrTerminalMode
    role: HerdrTerminalRole
    takeover: boolean
    target: string
  }) => void
  onPaneId?: (paneId: string | null | undefined) => void
}

export interface HerdrTerminalTransport extends TerminalTransport {
  detach(): void
  detachSession(): string | null
}

export function createHerdrTerminalTransport(
  options: HerdrTerminalTransportOptions
): HerdrTerminalTransport {
  const {
    terminalId,
    paneId = null,
    // Visible terminal ownership is control+takeover by default.
    mode: initialMode = "control",
    takeover: initialTakeover = true,
    sessionName = null,
    paneScrollEnabled,
    scrollEnabled,
    terminalScrollEnabled,
    applicationWheelEnabled,
    mouseEnabled,
    onPaneScroll,
    paneScrollController,
    onAttachment,
    onPaneId
  } = options

  let sessionId: string | null = null
  let mode: HerdrTerminalMode = initialMode
  let role: HerdrTerminalRole = initialMode === "control" ? "controller" : "observer"
  let takeover = initialMode === "control" ? initialTakeover : false
  let lastSeq: number | null = null
  let lastCols = 80
  let lastRows = 24
  let eventHandler: ((event: TerminalTransportEvent) => void) | null = null
  let openGeneration = 0
  /** Permanent disposal — survives release and blocks all reopen paths. */
  let disposed = false
  type InputQueue = { frames: Array<{ text: string; paste: boolean; bytes: number }>; bytes: number; drain: Promise<void> | null }
  let inputQueue: InputQueue | null = null
  /**
   * Each command is a separate IPC that may run concurrently, so pointer
   * events wait for the previous one: HERDR must see down, drag, up in order.
   */
  let mouseQueue: Array<{ action: HerdrMouseAction; cell: TerminalCell; modifiers: number }> = []
  let mouseDrain: Promise<void> | null = null
  let pendingScrollDelta = 0
  let pendingScrollCell: TerminalCell | undefined
  /** Wheel events behind pendingScrollDelta; an application gets one report each. */
  let pendingScrollEvents = 0
  /** The shared pane controller had no host range for the queued wheel. */
  let pendingApplicationWheel = false
  let scrollDrain: Promise<void> | null = null
  let scrollDrainToken: symbol | null = null
  let scrollDrainGeneration = 0
  // A rejected legacy connector command is not safe to retry: some bridges
  // close the connector while processing terminal.scroll. Trip the breaker
  // for this attachment and let a reconnect renegotiate capabilities.
  let terminalScrollUnavailable = false
  let paneScrollCache: { state: PaneScrollInfo; at: number } | null = null
  const applicationWheelAllowed = () => applicationWheelEnabled?.() === true && !terminalScrollUnavailable
  let frameWaiter: ((framed?: boolean) => void) | null = null
  /** Resolves true on the next frame, false after SCROLL_FRAME_WAIT_MS. Arm before sending. */
  const nextFrame = () => new Promise<boolean>((resolve) => {
    const done = (framed = false) => {
      clearTimeout(timer)
      if (frameWaiter === done) frameWaiter = null
      resolve(framed)
    }
    const timer = setTimeout(done, SCROLL_FRAME_WAIT_MS)
    frameWaiter?.()
    frameWaiter = done
  })
  const clearPaneScrollCache = () => { paneScrollCache = null }
  /** Wheels queued behind a failed drain must not leak into the next gesture. */
  const discardPendingWheel = () => {
    pendingScrollDelta = 0
    pendingScrollCell = undefined
    pendingScrollEvents = 0
    pendingApplicationWheel = false
  }
  const discardScroll = () => {
    frameWaiter?.()
    discardPendingWheel()
    scrollDrainGeneration += 1
    clearPaneScrollCache()
    scrollDrain = null
    scrollDrainToken = null
    paneScrollController?.()?.reset()
  }
  const discardInput = () => {
    if (inputQueue) { inputQueue.frames = []; inputQueue.bytes = 0 }
    inputQueue = null
    mouseQueue = []
  }
  const failInput = (queue: InputQueue, message: string) => {
    if (inputQueue !== queue) return
    discardInput()
    eventHandler?.({ type: "error", message })
  }

  const target = () => terminalId

  const mapEvent = (
    event: HerdrTerminalEvent,
    onEvent: (event: TerminalTransportEvent) => void
  ) => {
    if (event.type === "frame") {
      const dimension = (value: number) => Number.isInteger(value) && value >= 1 && value <= 1000
      const absent = (event.width === undefined || event.width === 0) && (event.height === undefined || event.height === 0)
      if (!(dimension(event.width) && dimension(event.height)) && (event.full || !absent)) {
        const attached = sessionId
        sessionId = null
        openGeneration++
        discardInput()
        discardScroll()
        if (attached) void herdrTerminalRelease(attached).catch(() => undefined)
        onEvent({ type: "error", code: "invalid-terminal-geometry", message: "Terminal frame dimensions exceed the supported limits" })
        return
      }
      // Backend already enforces first-full + contiguous; still ignore exact dups.
      if (lastSeq !== null && event.seq <= lastSeq) return
      lastSeq = event.seq
      frameWaiter?.(true)
      paneScrollController?.()?.frame()
      onEvent({
        type: "output",
        data: decodeFrameBytes(event.bytesBase64),
        seq: event.seq,
        droppedBytes: 0,
        truncated: false,
        full: event.full,
        width: event.width,
        height: event.height,
      })
      return
    }
    if (event.type === "closed") {
      onEvent({ type: "exit", code: null, reason: event.reason ?? null })
      return
    }
    if (event.type === "resync") {
      onEvent({ type: "resync", message: event.message })
      return
    }
    if (event.type === "error") {
      onEvent({ type: "error", message: event.message, code: event.code })
    }
  }

  const openConnector = async (
    nextMode: HerdrTerminalMode,
    nextTakeover: boolean,
    cols: number,
    rows: number,
    onEvent: (event: TerminalTransportEvent) => void
  ) => {
    if (disposed) return
    discardInput()
    discardScroll()
    terminalScrollUnavailable = false
    const generation = ++openGeneration
    lastSeq = null
    lastCols = cols
    lastRows = rows
    eventHandler = onEvent
    const result = await herdrTerminalOpen({
      target: target(),
      mode: nextMode,
      takeover: nextTakeover,
      cols,
      rows,
      sessionName,
      onEvent: (event) => {
        if (disposed || generation !== openGeneration) return
        // A dead remote stream retires this attachment like a close, but the
        // pane is alive: the page reopens instead of collapsing the leaf.
        if (event.type === "closed" || (event.type === "error" && (event.code === "host-stream-closed" || event.code === "invalid-terminal-geometry"))) {
          discardInput()
          discardScroll()
          sessionId = null
          lastSeq = null
          openGeneration += 1
        }
        mapEvent(event, onEvent)
      }
    })
    // Late open after release/dispose/unmount: drop connector, never re-register.
    if (disposed || generation !== openGeneration) {
      await herdrTerminalRelease(result.sessionId).catch(() => undefined)
      return
    }
    sessionId = result.sessionId
    inputQueue = { frames: [], bytes: 0, drain: null }
    mode = result.mode
    role = result.role
    takeover = result.takeover
    onAttachment?.({
      sessionId: result.sessionId,
      mode: result.mode,
      role: result.role,
      takeover: result.takeover,
      target: result.target
    })
    onPaneId?.(paneId)
    onEvent({ type: "control", mode: result.mode, role: result.role })
  }

  const enqueueInput = async (data: string, paste: boolean) => {
    if (disposed || !sessionId || mode !== "control" || !inputQueue || !data) return
    const queue = inputQueue
    const limit = 256 * 1024
    const bytes = data.length > limit
      ? limit + 1
      : data.length === 1 && data.charCodeAt(0) < 0x80 ? 1 : inputEncoder.encode(data).length
    if (queue.bytes + bytes > limit) {
      failInput(queue, "terminal-input-limit")
      throw new Error("terminal-input-limit")
    }
    const previous = queue.frames.at(-1)
    // HERDR recognizes paste only when the entire request is one bracketed
    // block. Never merge a paste with typing or with another paste.
    if (!paste && previous && !previous.paste) {
      previous.text += data
      previous.bytes += bytes
    } else queue.frames.push({ text: data, paste, bytes })
    queue.bytes += bytes
    if (!queue.drain) {
      const id = sessionId
      queue.drain = (async () => {
        await Promise.resolve()
        try {
          while (inputQueue === queue && queue.frames.length) {
            const frame = queue.frames.shift()!
            queue.bytes -= frame.bytes
            await herdrTerminalInput(id, frame.text, null)
          }
        } catch (error) {
          // Delivery may be unknown. Discard the unsent tail, never replay it.
          failInput(queue, "terminal-input-failed")
          throw error
        } finally { queue.drain = null }
      })()
    }
    await queue.drain
  }

  return {
    async open({ cols, rows, onEvent }) {
      if (disposed) return
      const openTakeover = mode === "control" ? takeover || initialTakeover : false
      await openConnector(mode, openTakeover, cols, rows, onEvent)
    },
    write: (data) => enqueueInput(data, false),
    paste: (text) => {
      const payload = text.replace(/\r\n?/g, "\n").replaceAll("\x1b[200~", "").replaceAll("\x1b[201~", "")
      return payload ? enqueueInput("\x1b[200~" + payload + "\x1b[201~", true) : Promise.resolve()
    },
    async resize(cols, rows) {
      if (disposed) return
      lastCols = cols
      lastRows = rows
      if (!sessionId) return
      // Resize is controller-owned in Herdr; observers skip silently.
      if (mode !== "control") return
      await herdrTerminalResize(sessionId, cols, rows)
    },
    async scroll(delta, cell) {
      if (
        disposed
        || !sessionId
        || mode !== "control"
        || delta === 0
        || scrollEnabled?.() === false
      ) return
      // A rejected connector command is terminal for this attachment unless
      // the caller supplied an addressable pane fallback. Preserve the
      // no-retry breaker for legacy runtimes that have neither transport.
      if (
        terminalScrollUnavailable
        && !(paneScrollEnabled?.() && paneId && sessionName)
      ) return
      // Wheel events can arrive faster than a remote host can acknowledge
      // them. Keep one request in flight and coalesce the rest so scrolls
      // cannot fill the same HERDR queue used by terminal input.
      const amount = Math.trunc(delta)
      if (!Number.isFinite(amount) || amount === 0) return
      const shared = paneScrollController?.()
      // HERDR routes a connector wheel by the child's modes: a mouse report
      // whenever the application enabled mouse reporting (even with host
      // history), alternate-scroll keys on the alternate screen, otherwise
      // host scrollback. The JSON connector does not expose those modes, so
      // the pane API is only the fallback where the command is unavailable.
      if (applicationWheelAllowed()) pendingApplicationWheel = true
      else if (shared && paneScrollEnabled?.() && (terminalScrollEnabled?.() !== true || terminalScrollUnavailable)) {
        recordHerdrTerminalMetric({ kind: "wheel", strategy: "pane", rows: Math.abs(amount) })
        shared.scroll(amount)
        return
      }
      shared?.follow()
      recordHerdrTerminalMetric({ kind: "wheel", strategy: "terminal", rows: Math.abs(amount) })
      pendingScrollDelta += amount
      pendingScrollEvents += 1
      pendingScrollCell = cell
      const generation = scrollDrainGeneration
      const activeSessionId = sessionId
      if (scrollDrain && scrollDrainGeneration === generation) return scrollDrain
      const drainToken = Symbol("scroll-drain")
      scrollDrainToken = drainToken
      const drain = (async () => {
        try {
          while (
            !disposed
            && generation === scrollDrainGeneration
            && sessionId === activeSessionId
            && mode === "control"
            && pendingScrollDelta !== 0
          ) {
            const nextDelta = pendingScrollDelta
            pendingScrollDelta = 0
            const cell = pendingScrollCell
            const events = Math.max(1, pendingScrollEvents)
            pendingScrollEvents = 0
            const applicationWheel = pendingApplicationWheel
            pendingApplicationWheel = false
            const direction = nextDelta < 0 ? "up" : "down"
            const lines = Math.max(1, Math.abs(nextDelta))
            // Native HERDR's connector command is the fast path. WSL uses the
            // pane API because terminal.scroll can tear down its bridge. Once
            // a native connector rejects, trip the breaker and use the pane
            // API for the remainder of this attachment.
            const usePaneScroll = !applicationWheel
              && paneScrollEnabled?.()
              && paneId
              && sessionName
              && (terminalScrollEnabled?.() !== true || terminalScrollUnavailable)
            if (usePaneScroll) {
              // A pane may legitimately have no scroll metadata yet (for
              // example before its first full frame). Keep the older
              // connector command as the compatible fallback instead of
              // turning a transient null into a visible wheel error.
              const now = Date.now()
              const cached = paneScrollCache && now - paneScrollCache.at <= PANE_SCROLL_CACHE_MS
                ? paneScrollCache.state
                : null
              const state = cached ?? await readPaneScroll(sessionName, paneId).catch(error => {
                if (terminalScrollEnabled?.() === false || terminalScrollUnavailable) throw error
                return null
              })
              if (generation !== scrollDrainGeneration || sessionId !== activeSessionId) return
              if (state) {
                const nextOffset = direction === "up"
                  ? Math.min(state.maxOffsetFromBottom, state.offsetFromBottom + lines)
                  : Math.max(0, state.offsetFromBottom - lines)
                const nextState = await setPaneScroll(sessionName, paneId, nextOffset)
                if (generation !== scrollDrainGeneration || sessionId !== activeSessionId) return
                paneScrollCache = {
                  state: nextState ?? { ...state, offsetFromBottom: nextOffset },
                  at: Date.now()
                }
                onPaneScroll?.(paneScrollCache.state)
                continue
              }
            }
            if ((terminalScrollEnabled?.() === false && !applicationWheel) || terminalScrollUnavailable) {
              throw new Error("pane-scroll-state-unavailable")
            }
            const reports = applicationWheel ? Math.min(events, lines, APPLICATION_WHEEL_REPORTS_PER_FRAME) : 1
            const rendered = nextFrame()
            const sentAt = performance.now()
            try {
              for (let report = 0; report < reports; report++) {
                // Split the rows so host scrollback still moves `lines` in total.
                const share = Math.floor(lines / reports) + (report < lines % reports ? 1 : 0)
                await timeHerdrTerminalIpc("terminal.scroll", () => cell
                  ? herdrTerminalScroll(activeSessionId, direction, share, cell)
                  : herdrTerminalScroll(activeSessionId, direction, share))
                if (generation !== scrollDrainGeneration || sessionId !== activeSessionId) return
              }
              const framed = await rendered
              recordHerdrTerminalMetric({ kind: "ipc", command: "terminal.scroll.frame", ms: performance.now() - sentAt, ok: framed })
            } catch (error) {
              frameWaiter?.()
              // A full remote stream queue is backpressure shared with input,
              // not a rejected command: drop the queued gestures, keep the route.
              if (herdrErrorKind(error) === "busy") {
                if (generation === scrollDrainGeneration) discardPendingWheel()
                return
              }
              terminalScrollUnavailable = true
              // Older connectors may reject terminal.scroll while still
              // supporting the pane API. Preserve the fallback for callers
              // that explicitly opted into pane scrolling after a capability
              // refresh.
              if (!paneScrollEnabled?.() || !paneId || !sessionName) throw error
              const state = await readPaneScroll(sessionName, paneId)
              if (generation !== scrollDrainGeneration || sessionId !== activeSessionId) return
              if (!state) throw error
              const nextOffset = direction === "up"
                ? Math.min(state.maxOffsetFromBottom, state.offsetFromBottom + lines)
                : Math.max(0, state.offsetFromBottom - lines)
              const nextState = await setPaneScroll(sessionName, paneId, nextOffset)
              if (generation !== scrollDrainGeneration || sessionId !== activeSessionId) return
              paneScrollCache = {
                state: nextState ?? { ...state, offsetFromBottom: nextOffset },
                at: Date.now()
              }
              onPaneScroll?.(paneScrollCache.state)
            }
          }
        } catch (error) {
          if (generation === scrollDrainGeneration) discardPendingWheel()
          throw error
        } finally {
          if (scrollDrainToken === drainToken) {
            scrollDrainToken = null
            scrollDrain = null
          }
        }
      })()
      scrollDrain = drain
      return drain
    },
    mouse(action, cell, modifiers) {
      if (disposed || !sessionId || mode !== "control" || mouseEnabled?.() !== true) return Promise.resolve()
      // Only the latest position of a queued drag matters.
      if (action === "drag" && mouseQueue.at(-1)?.action === "drag") mouseQueue.pop()
      mouseQueue.push({ action, cell, modifiers })
      mouseDrain ??= (async () => {
        await Promise.resolve()
        try {
          while (mouseQueue.length) {
            const id = sessionId
            if (disposed || !id || mode !== "control") break
            const event = mouseQueue.shift()!
            await herdrTerminalMouse(id, event.action, event.cell, event.modifiers)
          }
        } catch {
          // Delivery is unknown; never replay part of a gesture.
        } finally {
          mouseQueue = []
          mouseDrain = null
        }
      })()
      return mouseDrain
    },
    detach() {
      discardInput()
      discardScroll()
      disposed = true
      openGeneration += 1
      eventHandler = null
      sessionId = null
      lastSeq = null
    },
    detachSession() {
      discardInput()
      discardScroll()
      openGeneration += 1
      const id = sessionId
      sessionId = null
      lastSeq = null
      return id
    },
    async release() {
      discardInput()
      discardScroll()
      openGeneration += 1
      if (!sessionId) return
      const id = sessionId
      sessionId = null
      lastSeq = null
      // Release is idempotent and never terminates the Herdr pane/process.
      await herdrTerminalRelease(id).catch(() => undefined)
    },
    async dispose() {
      discardInput()
      discardScroll()
      disposed = true
      openGeneration += 1
      eventHandler = null
      if (!sessionId) return
      const id = sessionId
      sessionId = null
      lastSeq = null
      await herdrTerminalRelease(id).catch(() => undefined)
    },
    async takeControl() {
      if (disposed) return
      if (mode === "control" && role === "controller" && inputQueue) return
      const onEvent = eventHandler
      if (!onEvent) {
        throw new Error("Herdr transport is not open")
      }
      // Explicit Take Control: release observer connector, reopen as control+takeover.
      discardInput()
      discardScroll()
      if (sessionId) {
        const previous = sessionId
        sessionId = null
        await herdrTerminalRelease(previous).catch(() => undefined)
      }
      // Re-check after awaited release — unmount may have disposed mid-flight.
      if (disposed) return
      await openConnector("control", true, lastCols, lastRows, onEvent)
    },
    canWrite: () => !disposed && mode === "control" && sessionId !== null && inputQueue !== null,
    getControlMode: () => mode,
    getRole: () => role,
    getSessionId: () => sessionId,
    isDisposed: () => disposed
  }
}

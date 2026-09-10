/** Transport seam for Herdr terminal pages. */

import {
  herdrTerminalInput,
  herdrTerminalOpen,
  herdrTerminalRelease,
  herdrTerminalResize,
  herdrTerminalScroll
} from "@/lib/herdrIpc"
import type {
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
}

export type TerminalTransportEvent =
  | TerminalTransportOutputEvent
  | { type: "exit"; code: number | null }
  | { type: "error"; message: string }
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
  scroll?(delta: number): Promise<void>
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

function decodeFrameBytes(bytesBase64: string): string {
  try {
    if (typeof atob === "function") {
      const binary = atob(bytesBase64)
      const bytes = new Uint8Array(binary.length)
      for (let i = 0; i < binary.length; i++) bytes[i] = binary.charCodeAt(i)
      if (typeof TextDecoder !== "undefined") {
        return new TextDecoder("utf-8", { fatal: false }).decode(bytes)
      }
      let out = ""
      for (let i = 0; i < bytes.length; i++) out += String.fromCharCode(bytes[i])
      return out
    }
  } catch {
    // fall through
  }
  return bytesBase64
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
  type InputQueue = { frames: Array<{ text: string; paste: boolean }>; bytes: number; drain: Promise<void> | null }
  let inputQueue: InputQueue | null = null
  const discardInput = () => {
    if (inputQueue) { inputQueue.frames = []; inputQueue.bytes = 0 }
    inputQueue = null
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
      // Backend already enforces first-full + contiguous; still ignore exact dups.
      if (lastSeq !== null && event.seq <= lastSeq) return
      lastSeq = event.seq
      onEvent({
        type: "output",
        data: decodeFrameBytes(event.bytesBase64),
        seq: event.seq,
        droppedBytes: 0,
        truncated: false,
        full: event.full
      })
      return
    }
    if (event.type === "closed") {
      onEvent({ type: "exit", code: null })
      return
    }
    if (event.type === "resync") {
      onEvent({ type: "resync", message: event.message })
      return
    }
    if (event.type === "error") {
      onEvent({ type: "error", message: event.message })
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
        if (event.type === "closed") {
          discardInput()
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
    const bytes = data.length > limit ? limit + 1 : new TextEncoder().encode(data).length
    if (queue.bytes + bytes > limit) {
      failInput(queue, "terminal-input-limit")
      throw new Error("terminal-input-limit")
    }
    const previous = queue.frames.at(-1)
    // HERDR recognizes paste only when the entire request is one bracketed
    // block. Never merge a paste with typing or with another paste.
    if (!paste && previous && !previous.paste) previous.text += data
    else queue.frames.push({ text: data, paste })
    queue.bytes += bytes
    if (!queue.drain) {
      const id = sessionId
      queue.drain = (async () => {
        await Promise.resolve()
        try {
          while (inputQueue === queue && queue.frames.length) {
            const frame = queue.frames.shift()!
            queue.bytes -= new TextEncoder().encode(frame.text).length
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
    async scroll(delta) {
      if (disposed || !sessionId || mode !== "control" || delta === 0) return
      const direction = delta < 0 ? "up" : "down"
      const lines = Math.max(1, Math.abs(Math.trunc(delta)))
      await herdrTerminalScroll(sessionId, direction, lines)
    },
    detach() {
      discardInput()
      disposed = true
      openGeneration += 1
      eventHandler = null
      sessionId = null
      lastSeq = null
    },
    detachSession() {
      discardInput()
      openGeneration += 1
      const id = sessionId
      sessionId = null
      lastSeq = null
      return id
    },
    async release() {
      discardInput()
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

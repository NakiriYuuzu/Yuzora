// yuzora-wsl-agents Pi adapter
// YUZORA_WSL_ADAPTER=pi
// YUZORA_WSL_ADAPTER_VERSION=0.1.0
// managed by yuzora-wsl-agents; do not edit
//
// Derived from Herdr v0.8.2 src/integration/assets/pi/herdr-agent-state.ts
// (Apache-2.0). Its lifecycle mapping is preserved, with current Pi
// ui_prompt_start/ui_prompt_end events added for blocking user prompts. Transport is
// the POSIX CLI reporter. Native Pi session ids are log-only diagnostics and are never sent to Herdr.

import { spawn, type ChildProcess } from "node:child_process"
import { homedir } from "node:os"
import { join } from "node:path"

const SOURCE = "yuzora:wsl:pi"
const AGENT = "pi"
const REPORTER_REAP_TIMEOUT_MS = 1000
const HERDR_ENV = process.env.HERDR_ENV
const paneId = process.env.HERDR_PANE_ID

type AgentState = "working" | "blocked" | "idle" | "unknown"

type SessionContext = {
  mode?: string
  isIdle?: () => boolean
  sessionManager?: {
    getSessionId?: () => unknown
    getSessionFile?: () => unknown
  }
}

type BlockedPayload = {
  active?: boolean
  label?: string
}

type UiPromptPayload = {
  kind?: string
  title?: string
}

type PiHost = {
  events?: {
    on?: (event: string, handler: (data?: BlockedPayload) => void) => void
  }
  on: (
    event: string,
    handler: (event: unknown, ctx?: SessionContext) => void | Promise<void>
  ) => void
}

function enabled() {
  return HERDR_ENV === "1" && !!paneId
}

function reporterPath() {
  if (process.env.YUZORA_HERDR_WSL_REPORT) return process.env.YUZORA_HERDR_WSL_REPORT
  const piDir = process.env.PI_CODING_AGENT_DIR || join(homedir(), ".pi", "agent")
  return join(piDir, "extensions", "yuzora-herdr-wsl-report")
}

function redactSessionId(id: unknown): string | null {
  if (typeof id !== "string" || id.length === 0) return null
  if (id.length <= 8) return `(redacted len=${id.length})`
  return `${id.slice(0, 4)}…${id.slice(-2)} (len=${id.length})`
}

function logSessionDiagnostic(id: unknown, path: unknown) {
  const redactedId = redactSessionId(id)
  if (!redactedId && !path) return
  const pathNote = typeof path === "string" && path.length > 0 ? " path=present" : ""
  process.stderr.write(
    `[yuzora-herdr-wsl] native session (log-only, non-resumable): ${redactedId || "none"}${pathNote}\n`
  )
}

type ReporterAttempt = {
  delivered: boolean
  reaped: boolean
}

function killReporterProcessTree(child: ChildProcess) {
  const pid = child.pid
  if (pid && process.platform !== "win32") {
    try {
      process.kill(-pid, "SIGKILL")
      return
    } catch {
      // Fall back to the direct child below. A detached POSIX reporter normally
      // owns the process group, including its WSLInterop herdr.exe proxy.
    }
  }
  try {
    child.kill("SIGKILL")
  } catch {
    // The bounded reap timer below activates the circuit breaker if close never arrives.
  }
}

function runReporter(args: string[], timeoutMs: number): Promise<ReporterAttempt> {
  return new Promise((resolve) => {
    let done = false
    let spawned = false
    let cleanupStarted = false
    let timedOut = false
    const timers: {
      timeout?: ReturnType<typeof setTimeout>
      reap?: ReturnType<typeof setTimeout>
    } = {}
    const finish = (result: ReporterAttempt) => {
      if (done) return
      done = true
      if (timers.timeout) clearTimeout(timers.timeout)
      if (timers.reap) clearTimeout(timers.reap)
      resolve(result)
    }

    let child: ChildProcess
    try {
      child = spawn(reporterPath(), args, {
        // Reporter diagnostics were never consumed; an unread pipe can block
        // the shell before it reaches its own bounded failure path.
        stdio: ["ignore", "ignore", "ignore"],
        env: process.env,
        // The adapter runs inside WSL. A dedicated process group lets timeout
        // cleanup include the reporter shell and its Windows herdr.exe proxy.
        detached: process.platform !== "win32"
      })
    } catch {
      finish({ delivered: false, reaped: true })
      return
    }

    const beginCleanup = () => {
      if (cleanupStarted || done) return
      cleanupStarted = true
      timedOut = true
      killReporterProcessTree(child)
      // Never resolve a timeout as retryable until the owned child has closed.
      // If SIGKILL cannot be confirmed, fail closed and disable future reports.
      timers.reap = setTimeout(() => {
        finish({ delivered: false, reaped: false })
      }, REPORTER_REAP_TIMEOUT_MS)
      timers.reap.unref()
    }

    child.once("spawn", () => {
      spawned = true
    })
    child.once("error", () => {
      if (!spawned) {
        finish({ delivered: false, reaped: true })
        return
      }
      beginCleanup()
    })
    child.once("close", (code) => {
      finish({ delivered: !timedOut && code === 0, reaped: true })
    })

    timers.timeout = setTimeout(beginCleanup, timeoutMs)
    timers.timeout.unref()
  })
}

let reporterDisabled = false
let reporterDisableLogged = false

function disableReporterAfterCleanupFailure() {
  reporterDisabled = true
  if (reporterDisableLogged) return
  reporterDisableLogged = true
  process.stderr.write(
    "[yuzora-herdr-wsl] reporter cleanup could not be confirmed; lifecycle reporting disabled for this Pi process\n"
  )
}

async function invokeReporter(args: string[]) {
  if (reporterDisabled) return
  const joined = args.join(" ")
  const sessionCmd = ["report-agent", "session"].join("-")
  if (joined.includes(sessionCmd) || /--agent-session(?:-id|-path)/.test(joined)) {
    process.stderr.write("[yuzora-herdr-wsl] refused session identity report\n")
    return
  }
  const first = await runReporter(args, 500)
  if (!first.reaped) {
    disableReporterAfterCleanupFailure()
    return
  }
  if (first.delivered) return
  const second = await runReporter(args, 1500)
  if (!second.reaped) disableReporterAfterCleanupFailure()
}

type QueuedState = {
  kind: "report" | "release"
  state?: AgentState
  message?: string
}

let sendInFlight = false
let queued: QueuedState | undefined
let drainWaiters: Array<() => void> = []

function queue(next: QueuedState) {
  queued = next
  if (!sendInFlight) void drain()
}

function waitForDrain(): Promise<void> {
  if (!sendInFlight && !queued) return Promise.resolve()
  return new Promise((resolve) => {
    drainWaiters.push(resolve)
  })
}

function resolveDrainWaiters() {
  const waiters = drainWaiters
  drainWaiters = []
  for (const resolve of waiters) resolve()
}

async function drain() {
  if (sendInFlight) return
  sendInFlight = true
  try {
    while (queued) {
      const next = queued
      queued = undefined
      if (next.kind === "release") {
        await invokeReporter(["release-agent"])
      } else {
        const args = ["report-agent", "--state", next.state || "unknown"]
        if (next.message) args.push("--message", next.message)
        await invokeReporter(args)
      }
    }
  } finally {
    sendInFlight = false
    if (queued) {
      void drain()
    } else {
      resolveDrainWaiters()
    }
  }
}

export default function (pi: PiHost) {
  if (!enabled()) return

  let agentActive = false
  let blockedCount = 0
  let blockedMessage: string | undefined
  let uiPromptDepth = 0
  let uiPromptMessage: string | undefined
  let lastState: AgentState | undefined
  let lastMessage: string | undefined
  let rootSession = false
  let released = false

  function desiredState(): { state: AgentState; message?: string } {
    if (blockedCount > 0 || uiPromptDepth > 0) {
      return { state: "blocked", message: blockedMessage || uiPromptMessage }
    }
    if (agentActive) return { state: "working", message: undefined }
    return { state: "idle", message: undefined }
  }

  function publishState(force = false) {
    if (released) return
    const next = desiredState()
    if (!force && next.state === lastState && next.message === lastMessage) return
    lastState = next.state
    lastMessage = next.message
    queue({ kind: "report", state: next.state, message: next.message })
  }

  function captureSession(ctx?: SessionContext) {
    let id: unknown
    let path: unknown
    try {
      id = ctx?.sessionManager?.getSessionId?.()
    } catch {
      id = undefined
    }
    try {
      path = ctx?.sessionManager?.getSessionFile?.()
    } catch {
      path = undefined
    }
    logSessionDiagnostic(id, path)
  }

  pi.events?.on?.("herdr:blocked", (data) => {
    if (!rootSession) return
    if (!data?.active) {
      blockedCount = Math.max(0, blockedCount - 1)
      if (blockedCount === 0) blockedMessage = undefined
      publishState()
      return
    }
    blockedCount += 1
    blockedMessage = data.label
    publishState()
  })

  pi.on("ui_prompt_start", (event) => {
    if (!rootSession) return
    const prompt = event as UiPromptPayload
    uiPromptDepth += 1
    uiPromptMessage = prompt.title || prompt.kind || "waiting for user"
    publishState()
  })

  pi.on("ui_prompt_end", () => {
    if (!rootSession) return
    uiPromptDepth = Math.max(0, uiPromptDepth - 1)
    if (uiPromptDepth === 0) uiPromptMessage = undefined
    publishState()
  })

  pi.on("session_start", (_event, ctx) => {
    if (ctx?.mode !== "tui") return
    rootSession = true
    captureSession(ctx)
    agentActive = ctx?.isIdle?.() === false
    publishState(true)
  })

  pi.on("agent_start", (_event, ctx) => {
    if (!rootSession) return
    captureSession(ctx)
    agentActive = true
    publishState()
  })

  pi.on("agent_settled", (_event, ctx) => {
    if (!rootSession || ctx?.isIdle?.() !== true) return
    agentActive = false
    publishState()
  })

  pi.on("session_shutdown", async () => {
    if (released || !rootSession) return
    released = true
    rootSession = false
    queue({ kind: "release" })
    await waitForDrain()
  })
}

export const __test__ = {
  SOURCE,
  AGENT,
  redactSessionId,
  reporterPath
}

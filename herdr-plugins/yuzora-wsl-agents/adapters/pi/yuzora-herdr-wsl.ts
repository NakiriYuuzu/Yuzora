// yuzora-wsl-agents Pi adapter
// YUZORA_WSL_ADAPTER=pi
// YUZORA_WSL_ADAPTER_VERSION=0.1.0
// managed by yuzora-wsl-agents; do not edit
//
// Derived from Herdr v0.8.2 src/integration/assets/pi/herdr-agent-state.ts
// (Apache-2.0). Its lifecycle mapping is preserved, with current Pi
// ui_prompt_start/ui_prompt_end events added for blocking user prompts. Transport is
// the POSIX CLI reporter. Native Pi session ids are log-only diagnostics and are never sent to Herdr.

import { spawn } from "node:child_process"
import { homedir } from "node:os"
import { join } from "node:path"

const SOURCE = "yuzora:wsl:pi"
const AGENT = "pi"
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
  on: (event: string, handler: (event: unknown, ctx?: SessionContext) => void) => void
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

function runReporter(args: string[], timeoutMs: number): Promise<boolean> {
  return new Promise((resolve) => {
    let done = false
    const finish = (ok: boolean) => {
      if (done) return
      done = true
      resolve(ok)
    }
    const child = spawn(reporterPath(), args, {
      stdio: ["ignore", "ignore", "pipe"],
      env: process.env
    })
    const timer = setTimeout(() => {
      child.kill()
      finish(false)
    }, timeoutMs)
    timer.unref()
    child.on("error", () => {
      clearTimeout(timer)
      finish(false)
    })
    child.on("exit", (code) => {
      clearTimeout(timer)
      finish(code === 0)
    })
  })
}

async function invokeReporter(args: string[]) {
  const joined = args.join(" ")
  const sessionCmd = ["report-agent", "session"].join("-")
  if (joined.includes(sessionCmd) || /--agent-session(?:-id|-path)/.test(joined)) {
    process.stderr.write("[yuzora-herdr-wsl] refused session identity report\n")
    return
  }
  if (await runReporter(args, 500)) return
  await runReporter(args, 1500)
}

type QueuedState = {
  kind: "report" | "release"
  state?: AgentState
  message?: string
}

let sendInFlight = false
let queued: QueuedState | undefined

function queue(next: QueuedState) {
  queued = next
  if (!sendInFlight) void drain()
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
    if (queued) void drain()
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

  const release = () => {
    if (released || !rootSession) return
    released = true
    queue({ kind: "release" })
  }
  process.on("beforeExit", release)
  process.on("exit", release)
}

export const __test__ = {
  SOURCE,
  AGENT,
  redactSessionId,
  reporterPath
}

import i18n from "@/lib/i18n"
import { hasHerdrMethod } from "@/lib/herdrCapabilities"
import type { HerdrAgentInfo, HerdrSessionRuntime, HerdrSnapshot } from "@/lib/herdrTypes"
import type { HerdrTask } from "@/state/herdrToolsStore"

export type HerdrAvailability = { ok: true } | { ok: false; reason: string }

export const AGENT_NAME_PATTERN = /^[a-z][a-z0-9_-]{0,31}$/
export const SESSION_NAME_PATTERN = /^[A-Za-z0-9_.-]{1,64}$/
const why = (key: string, options?: Record<string, unknown>): HerdrAvailability => ({ ok: false, reason: i18n.t(`herdrTools:reason.${key}`, options) })

/** Why a typed HERDR operation cannot run now; `starting` marks the default Session while Yuzora boots it. */
export function herdrActionAvailability(runtime: HerdrSessionRuntime | null | undefined, method: string | null, starting = false): HerdrAvailability {
  if (runtime?.connectionState !== "ready") return why(starting || runtime?.connectionState === "connecting" ? "sessionStarting" : "sessionNotConnected")
  if (!runtime.capabilities?.server.running) return why("serverNotRunning")
  if (method && !hasHerdrMethod(runtime.capabilities, method)) {
    const version = runtime.capabilities.server.version ?? runtime.capabilities.binaryVersion
    return version ? why("unsupported", { version }) : why("unsupportedUnknown")
  }
  return { ok: true }
}

export function agentPanes(snapshot: HerdrSnapshot | null | undefined): HerdrAgentInfo[] {
  return snapshot?.agents.filter(agent => agent.paneId) ?? []
}
export function paneHasAgent(snapshot: HerdrSnapshot | null | undefined, paneId?: string | null): boolean {
  return Boolean(paneId) && agentPanes(snapshot).some(agent => agent.paneId === paneId)
}
/** A pane that already runs an agent is messaged; any other pane starts one. */
export function agentTaskForPane(snapshot: HerdrSnapshot | null | undefined, paneId?: string | null): "messageAgent" | "startAgent" {
  return paneHasAgent(snapshot, paneId) ? "messageAgent" : "startAgent"
}

const taskMethods: Record<HerdrTask, string | null> = {
  worktree: "worktree.create", startAgent: "agent.start", messageAgent: "agent.prompt", movePane: "pane.move",
  sessions: null, integrations: "integration.list", plugins: "plugin.list",
}
/** Availability of a task for the selected Session, including the Space/Pane/Agent it needs. */
export function herdrTaskAvailability(task: HerdrTask, runtime: HerdrSessionRuntime | null | undefined, starting = false): HerdrAvailability {
  // Session management is how a stopped or missing Session gets fixed, so it never waits on a runtime.
  if (task === "sessions") return { ok: true }
  const base = herdrActionAvailability(runtime, taskMethods[task], starting)
  if (!base.ok) return base
  const snapshot = runtime?.snapshot
  if (task === "worktree" && !snapshot?.spaces.length) return why("noSpace")
  if ((task === "startAgent" || task === "movePane") && !snapshot?.terminals.some(pane => pane.paneId)) return why("noPane")
  if (task === "messageAgent" && !agentPanes(snapshot).length) return why("noAgent")
  return { ok: true }
}

export function herdrNativeAvailability(runtime: HerdrSessionRuntime | null | undefined, starting = false): HerdrAvailability {
  const base = herdrActionAvailability(runtime, null, starting)
  if (!base.ok) return base
  if (runtime?.capabilities?.api.snapshot && runtime.capabilities.server.compatible === true) return { ok: true }
  return why("unsupported", { version: runtime?.capabilities?.server.version ?? runtime?.capabilities?.binaryVersion ?? "?" })
}

/** `<kind>` or `<kind>-<n>`, unique among the Session's agent names. */
export function suggestAgentName(kind: string, snapshot: HerdrSnapshot | null | undefined): string {
  const taken = new Set(snapshot?.agents.map(agent => agent.name))
  if (!taken.has(kind)) return kind
  for (let n = 2; ; n++) if (!taken.has(`${kind}-${n}`)) return `${kind}-${n}`
}

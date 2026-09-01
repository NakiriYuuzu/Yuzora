export type AgentState = "working" | "idle" | "blocked" | "unknown"

export type LifecycleSnapshot = {
  rootSession: boolean
  agentActive: boolean
  blockedCount: number
  blockedMessage?: string
}

export function initialLifecycle(): LifecycleSnapshot {
  return {
    rootSession: false,
    agentActive: false,
    blockedCount: 0,
    blockedMessage: undefined
  }
}

export function desiredState(snapshot: LifecycleSnapshot): {
  state: AgentState
  message?: string
} {
  if (!snapshot.rootSession) return { state: "unknown" }
  if (snapshot.blockedCount > 0) {
    return { state: "blocked", message: snapshot.blockedMessage }
  }
  if (snapshot.agentActive) return { state: "working" }
  return { state: "idle" }
}

export function applySessionStart(
  snapshot: LifecycleSnapshot,
  mode: string | undefined,
  isIdle: boolean | undefined
): LifecycleSnapshot {
  if (mode !== "tui") return snapshot
  return {
    ...snapshot,
    rootSession: true,
    agentActive: isIdle === false
  }
}

export function applyAgentStart(snapshot: LifecycleSnapshot): LifecycleSnapshot {
  if (!snapshot.rootSession) return snapshot
  return { ...snapshot, agentActive: true }
}

export function applyAgentSettled(
  snapshot: LifecycleSnapshot,
  isIdle: boolean | undefined
): LifecycleSnapshot {
  if (!snapshot.rootSession || isIdle !== true) return snapshot
  return { ...snapshot, agentActive: false }
}

export function applyBlocked(
  snapshot: LifecycleSnapshot,
  active: boolean,
  label?: string
): LifecycleSnapshot {
  if (!snapshot.rootSession) return snapshot
  if (!active) {
    const blockedCount = Math.max(0, snapshot.blockedCount - 1)
    return {
      ...snapshot,
      blockedCount,
      blockedMessage: blockedCount === 0 ? undefined : snapshot.blockedMessage
    }
  }
  return {
    ...snapshot,
    blockedCount: snapshot.blockedCount + 1,
    blockedMessage: label
  }
}

export function redactSessionId(id: string): string {
  if (id.length <= 8) return `(redacted len=${id.length})`
  return `${id.slice(0, 4)}…${id.slice(-2)} (len=${id.length})`
}

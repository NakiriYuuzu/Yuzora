import type { AgentLatestVersion } from "@/lib/ipc"
import type { AgentId } from "@/lib/agentPresets"

export const AGENT_VERSION_STORAGE_KEY = "yuzora:agent-versions"

export type AgentVersionMap = Partial<Record<AgentId, string>>

export interface AgentUpdate {
  currentVersion: string
  latestVersion: string
}

const AGENT_IDS: AgentId[] = ["pi", "claude", "codex"]

export function loadAgentVersions(): AgentVersionMap {
  try {
    const raw = localStorage.getItem(AGENT_VERSION_STORAGE_KEY)
    const parsed = raw ? JSON.parse(raw) as Record<string, unknown> : {}
    const versions: AgentVersionMap = {}
    for (const agentId of AGENT_IDS) {
      const version = normalizeAgentVersion(parsed[agentId])
      if (version) versions[agentId] = version
    }
    return versions
  } catch {
    return {}
  }
}

export function rememberAgentVersion(agentId: AgentId, value: unknown): void {
  const version = normalizeAgentVersion(value)
  if (!version) return
  try {
    localStorage.setItem(AGENT_VERSION_STORAGE_KEY, JSON.stringify({
      ...loadAgentVersions(),
      [agentId]: version,
    }))
  } catch {
    // Private mode / quota: the live session still carries the version.
  }
}

export function indexLatestAgentVersions(rows: AgentLatestVersion[]): AgentVersionMap {
  const versions: AgentVersionMap = {}
  for (const row of rows) {
    if (!AGENT_IDS.includes(row.agentId)) continue
    const version = normalizeAgentVersion(row.version)
    if (version) versions[row.agentId] = version
  }
  return versions
}

export function agentUpdateFor(
  agentId: AgentId,
  currentVersions: AgentVersionMap,
  latestVersions: AgentVersionMap,
): AgentUpdate | null {
  const currentVersion = normalizeAgentVersion(currentVersions[agentId])
  const latestVersion = normalizeAgentVersion(latestVersions[agentId])
  if (!currentVersion || !latestVersion || currentVersion === latestVersion) return null
  return { currentVersion, latestVersion }
}

function normalizeAgentVersion(value: unknown): string | null {
  if (typeof value !== "string") return null
  const trimmed = value.trim()
  if (!trimmed) return null
  return /^v\d/.test(trimmed) ? trimmed.slice(1) : trimmed
}

import { create } from "zustand"

/**
 * Session-scoped Agent recency for Alt+Tab-style cycling. In memory only:
 * Herdr Agent ids are runtime identities and are not persisted across restarts.
 */
export const AGENT_MRU_LIMIT = 50

export function agentMruKey(sessionName: string, agentId: string): string {
  return JSON.stringify([sessionName, agentId])
}

interface AgentMruState {
  keys: string[]
  touch: (sessionName: string, agentId: string) => void
}

export const useAgentMruStore = create<AgentMruState>((set) => ({
  keys: [],
  touch: (sessionName, agentId) => set((state) => {
    const key = agentMruKey(sessionName, agentId)
    if (state.keys[0] === key) return state
    return { keys: [key, ...state.keys.filter((item) => item !== key)].slice(0, AGENT_MRU_LIMIT) }
  }),
}))

/**
 * Most recently used Agents first (only those that still exist), followed by
 * the remaining Agents in their list order.
 */
export function orderAgentsByRecency<T>(agents: readonly T[], keyOf: (agent: T) => string, recent: readonly string[]): T[] {
  const byKey = new Map(agents.map((agent) => [keyOf(agent), agent] as const))
  const seen = new Set<string>()
  const ordered: T[] = []
  for (const key of recent) {
    const agent = byKey.get(key)
    if (agent && !seen.has(key)) { ordered.push(agent); seen.add(key) }
  }
  for (const agent of agents) {
    const key = keyOf(agent)
    if (!seen.has(key)) { ordered.push(agent); seen.add(key) }
  }
  return ordered
}

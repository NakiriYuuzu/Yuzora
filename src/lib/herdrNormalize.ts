import type {
  HerdrAgentInfo,
  HerdrAgentStatus,
  HerdrSnapshot,
  HerdrSnapshotResult,
  HerdrSpaceInfo,
  HerdrTabInfo,
  HerdrTerminalInfo
} from "./herdrTypes"
import { spaceProvenanceFromSnapshotWorktree } from "./herdrWorktree"

/** Legacy page namespace alias; backend resolves `live` to the default named session. */
export const HERDR_LIVE_SESSION_ID = "live"

function asRecord(value: unknown): Record<string, unknown> | null {
  if (value && typeof value === "object" && !Array.isArray(value)) {
    return value as Record<string, unknown>
  }
  return null
}

function asArray(value: unknown): unknown[] {
  return Array.isArray(value) ? value : []
}

function asString(value: unknown): string | null {
  return typeof value === "string" && value.length > 0 ? value : null
}

function asNumber(value: unknown): number | null {
  return typeof value === "number" && Number.isFinite(value) ? value : null
}

function asBool(value: unknown): boolean {
  return value === true
}

function asAgentStatus(value: unknown): HerdrAgentStatus {
  switch (value) {
    case "idle":
    case "working":
    case "blocked":
    case "done":
    case "unknown":
      return value
    default:
      return "unknown"
  }
}

/**
 * Normalize optional presentation metadata without allowing it to affect any
 * Herdr resource identity, routing, or workspace-path decisions.
 */
export function normalizeHerdrExecutionOrigin(value: unknown) {
  const origin = asRecord(value)
  if (origin?.kind !== "wsl") return undefined

  const rawDistribution = origin.distribution
  if (typeof rawDistribution !== "string") return { kind: "wsl" } as const
  const distribution = rawDistribution.trim()
  const characters = Array.from(distribution)
  const hasControlCharacter = characters.some((character) => {
    const codePoint = character.codePointAt(0) ?? 0
    return codePoint <= 0x1f || (codePoint >= 0x7f && codePoint <= 0x9f)
  })
  if (!distribution || characters.length > 128 || hasControlCharacter) {
    return { kind: "wsl" } as const
  }
  return { kind: "wsl", distribution } as const
}

/** Format normalized presentation metadata for a compact Agent badge. */
export function formatHerdrExecutionOrigin(
  origin: ReturnType<typeof normalizeHerdrExecutionOrigin>
): string | null {
  if (!origin) return null
  return origin.distribution ? `WSL · ${origin.distribution}` : "WSL"
}

/**
 * Normalize raw Herdr `session.snapshot` payload into ADE Spaces/Agents/Panes.
 * Unknown / extra wire fields are ignored defensively.
 */
export function normalizeHerdrSnapshot(
  result: HerdrSnapshotResult,
  herdrSessionId: string = HERDR_LIVE_SESSION_ID
): HerdrSnapshot {
  const raw = result.snapshot
  const root = asRecord(raw) ?? {}

  const workspaces = asArray(root.workspaces)
  const agentsRaw = asArray(root.agents)
  const tabsRaw = asArray(root.tabs)
  const panesRaw = asArray(root.panes)
  const focusedWorkspaceId = asString(root.focused_workspace_id)
  const focusedTabId = asString(root.focused_tab_id)
  const focusedPaneId = asString(root.focused_pane_id)

  // Protocol 19 workspace summaries do not expose cwd/path. Derive the Space
  // path from its agent/pane launch cwd so selecting another Space can also
  // switch Yuzora's project context. Prefer `cwd` over mutable foreground_cwd.
  const fallbackSpacePaths = new Map<string, string>()
  for (const values of [agentsRaw, panesRaw]) {
    for (const value of values) {
      const item = asRecord(value)
      if (!item) continue
      const workspaceId = asString(item.workspace_id)
      const path = asString(item.cwd) ?? asString(item.foreground_cwd)
      if (workspaceId && path && !fallbackSpacePaths.has(workspaceId)) {
        fallbackSpacePaths.set(workspaceId, path)
      }
    }
  }

  const spaces: HerdrSpaceInfo[] = []
  for (let i = 0; i < workspaces.length; i++) {
    const ws = asRecord(workspaces[i])
    if (!ws) continue
    const id = asString(ws.workspace_id) ?? asString(ws.id)
    if (!id) continue
    const order = asNumber(ws.number) ?? i
    const worktreeRec = asRecord(ws.worktree)
    const snapshotProvenance = spaceProvenanceFromSnapshotWorktree(worktreeRec)
    // Protocol 19 path fallback: worktree.checkout_path → path/cwd → agent/pane cwd.
    const path =
      snapshotProvenance.path ??
      asString(ws.path) ??
      asString(ws.cwd) ??
      fallbackSpacePaths.get(id) ??
      null
    spaces.push({
      id,
      label: asString(ws.label) ?? id,
      order,
      focused: asBool(ws.focused),
      activeTabId: asString(ws.active_tab_id),
      path,
      status: asAgentStatus(ws.agent_status),
      agentCount: undefined,
      terminalCount: asNumber(ws.pane_count) ?? undefined,
      tabCount: asNumber(ws.tab_count) ?? undefined,
      repoKey: snapshotProvenance.repoKey ?? null,
      repoName: snapshotProvenance.repoName ?? null,
      repoRoot: snapshotProvenance.repoRoot ?? null,
      isLinkedWorktree: snapshotProvenance.isLinkedWorktree ?? null
    })
  }
  spaces.sort((a, b) => a.order - b.order)

  const paneExecutionOrigins = new Map<string, ReturnType<typeof normalizeHerdrExecutionOrigin>>()
  for (const paneValue of panesRaw) {
    const pane = asRecord(paneValue)
    const paneId = pane ? asString(pane.pane_id) : null
    const executionOrigin = pane ? normalizeHerdrExecutionOrigin(pane.execution_origin) : undefined
    if (paneId && executionOrigin) paneExecutionOrigins.set(paneId, executionOrigin)
  }

  const agents: HerdrAgentInfo[] = []
  for (let i = 0; i < agentsRaw.length; i++) {
    const agent = asRecord(agentsRaw[i])
    if (!agent) continue
    const terminalId = asString(agent.terminal_id)
    const workspaceId = asString(agent.workspace_id)
    const paneId = asString(agent.pane_id)
    if (!workspaceId || !paneId) continue
    const name =
      asString(agent.display_agent) ??
      asString(agent.agent) ??
      asString(agent.name) ??
      asString(agent.title) ??
      terminalId ??
      paneId
    agents.push({
      id: terminalId ?? paneId,
      name,
      status: asAgentStatus(agent.agent_status),
      workspaceId,
      tabId: asString(agent.tab_id),
      paneId,
      terminalId,
      title:
        asString(agent.title) ??
        asString(agent.terminal_title_stripped) ??
        asString(agent.terminal_title) ??
        asString(agent.name),
      displayAgent: asString(agent.display_agent) ?? asString(agent.agent),
      focused: asBool(agent.focused),
      executionOrigin: Object.hasOwn(agent, "execution_origin")
        ? normalizeHerdrExecutionOrigin(agent.execution_origin)
        : paneExecutionOrigins.get(paneId)
    })
  }

  const terminals: HerdrTerminalInfo[] = []
  for (const paneValue of panesRaw) {
    const pane = asRecord(paneValue)
    if (!pane) continue
    const terminalId = asString(pane.terminal_id)
    const paneId = asString(pane.pane_id)
    if (!terminalId && !paneId) continue
    terminals.push({
      terminalId: terminalId ?? paneId!,
      paneId,
      workspaceId: asString(pane.workspace_id),
      tabId: asString(pane.tab_id),
      title:
        asString(pane.title) ??
        asString(pane.label) ??
        asString(pane.terminal_title_stripped) ??
        asString(pane.terminal_title),
      cwd: asString(pane.cwd) ?? asString(pane.foreground_cwd),
      status: asAgentStatus(pane.agent_status),
      executionOrigin: normalizeHerdrExecutionOrigin(pane.execution_origin)
    })
  }

  const activeTabsByWorkspace = new Map(
    spaces.flatMap((space) =>
      space.activeTabId ? ([[space.id, space.activeTabId]] as const) : []
    )
  )
  const tabs: HerdrTabInfo[] = []
  const knownTabIds = new Set<string>()

  const appendTab = (
    id: string,
    workspaceId: string,
    label: string,
    order: number,
    paneCount: number,
    status: HerdrAgentStatus,
    wireFocused: boolean
  ) => {
    if (knownTabIds.has(id)) return
    knownTabIds.add(id)
    const terminalCandidates = terminals.filter((terminal) => terminal.tabId === id)
    const agentCandidates = agents.filter((agent) => agent.tabId === id)
    const representativeTerminal =
      terminalCandidates.find((terminal) => terminal.paneId === focusedPaneId) ??
      terminalCandidates[0] ??
      agentCandidates[0] ??
      null
    tabs.push({
      id,
      label,
      order,
      workspaceId,
      paneCount,
      status,
      active: activeTabsByWorkspace.get(workspaceId) === id,
      focused: wireFocused || focusedTabId === id,
      paneId: representativeTerminal?.paneId ?? null,
      terminalId: representativeTerminal?.terminalId ?? null,
      sessionName: herdrSessionId
    })
  }

  for (let i = 0; i < tabsRaw.length; i++) {
    const tab = asRecord(tabsRaw[i])
    if (!tab) continue
    const id = asString(tab.tab_id) ?? asString(tab.id)
    const workspaceId = asString(tab.workspace_id)
    if (!id || !workspaceId) continue
    appendTab(
      id,
      workspaceId,
      asString(tab.label) ?? id,
      asNumber(tab.number) ?? i,
      asNumber(tab.pane_count) ?? terminals.filter((terminal) => terminal.tabId === id).length,
      asAgentStatus(tab.agent_status),
      asBool(tab.focused)
    )
  }

  // Defensive fallback for older/partial snapshots that omit `tabs[]`.
  for (const source of [...terminals, ...agents]) {
    const id = source.tabId
    const workspaceId = source.workspaceId
    if (!id || !workspaceId || knownTabIds.has(id)) continue
    appendTab(
      id,
      workspaceId,
      source.title ?? id,
      tabs.length,
      terminals.filter((terminal) => terminal.tabId === id).length || 1,
      source.status ?? "unknown",
      focusedTabId === id
    )
  }
  tabs.sort((a, b) => {
    if (a.workspaceId === b.workspaceId) return a.order - b.order
    return a.workspaceId.localeCompare(b.workspaceId)
  })

  // Roll agent counts onto spaces when not present on the wire.
  if (spaces.length > 0 && agents.length > 0) {
    const counts = new Map<string, number>()
    for (const agent of agents) {
      counts.set(agent.workspaceId, (counts.get(agent.workspaceId) ?? 0) + 1)
    }
    for (const space of spaces) {
      space.agentCount = counts.get(space.id) ?? 0
    }
  }

  const spaceLabels = new Map(spaces.map((space) => [space.id, space.label]))
  for (const agent of agents) {
    agent.sessionName = herdrSessionId
    agent.spaceLabel = spaceLabels.get(agent.workspaceId) ?? agent.workspaceId
  }

  const resolvedFocusedWorkspaceId =
    focusedWorkspaceId ?? spaces.find((s) => s.focused)?.id ?? null
  const focusedTerminal =
    terminals.find((t) => t.paneId && t.paneId === focusedPaneId)?.terminalId ??
    agents.find((a) => a.focused)?.terminalId ??
    null

  return {
    herdrSessionId,
    protocol: result.protocol,
    version: result.version,
    spaces,
    agents,
    tabs,
    terminals,
    focusedWorkspaceId: resolvedFocusedWorkspaceId,
    focusedTabId,
    focusedPaneId,
    focusedTerminalId: focusedTerminal,
    revision: null,
    raw
  }
}

export type AgentId = "pi" | "claude" | "codex"
export type AgentPreset = AgentId | "custom"
export type AgentCommandMode = "verified" | "latest" | "custom"

export interface AgentDescriptor {
  id: AgentId
  label: string
  verifiedCommand: string
  latestCommand: string
}

export interface AgentCommandIdentity {
  selectedPreset: AgentPreset
  commandMode: AgentCommandMode
  trustedAgentId: AgentId | null
}

export interface AgentCommandResolution extends AgentCommandIdentity {
  command: string
}

// Verified 是預設與可重現 baseline；Latest 只能由使用者明確選擇。
export const AGENT_PRESETS: AgentDescriptor[] = [
  {
    id: "pi",
    label: "Pi",
    verifiedCommand: "bunx pi-acp@0.0.31",
    latestCommand: "bunx pi-acp@latest",
  },
  {
    id: "claude",
    label: "Claude",
    verifiedCommand: "bunx @agentclientprotocol/claude-agent-acp@0.58.1",
    latestCommand: "bunx @agentclientprotocol/claude-agent-acp@latest",
  },
  {
    id: "codex",
    label: "Codex",
    verifiedCommand: "bunx @agentclientprotocol/codex-acp@1.1.2",
    latestCommand: "bunx @agentclientprotocol/codex-acp@latest",
  },
]

export const DEFAULT_AGENT_ID: AgentId = "pi"
export const DEFAULT_AGENT_COMMAND = AGENT_PRESETS[0].verifiedCommand

// agentId → 品牌色 token 的單一對照（single source of truth）。消費點
// （AgentZonePanel 的 header avatar/chip、AgentNavContent 的 row badge）
// 皆從這裡衍生視覺，不各自硬編一份 label/glyph/color，避免像 "Pi" vs "pi" 那樣漂移。
export const AGENT_VISUALS: Record<AgentId, { label: string; glyph: string; colorVar: string; softVar: string }> = {
  pi: { label: "Pi", glyph: "π", colorVar: "var(--agent-pi)", softVar: "var(--agent-pi-soft)" },
  claude: { label: "Claude", glyph: "C", colorVar: "var(--agent-claude)", softVar: "var(--agent-claude-soft)" },
  codex: { label: "Codex", glyph: "X", colorVar: "var(--agent-codex)", softVar: "var(--agent-codex-soft)" },
}

// custom agent 沒有固定 label/glyph——由呼叫端以 session.agentLabel 決定顯示文字。
export const CUSTOM_AGENT_VISUAL = { colorVar: "var(--agent-custom)", softVar: "var(--agent-custom-soft)" }

// custom／undefined／未知 agentId 的顯示名 fallback：一律以 agentLabel（trim 後
// 非空）為準，否則用呼叫端提供的 fallback 字串。header（AgentZonePanel）與 nav
// row（AgentNavContent）共用此語意，避免各自硬編出不同的 fallback 行為。
export function agentDisplayName(
  agentId: AgentPreset | undefined,
  agentLabel: string,
  fallback: string
): string {
  const known = agentId && agentId !== "custom" ? AGENT_VISUALS[agentId] : undefined
  if (known) return known.label
  return agentLabel.trim() || fallback
}

export function commandForAgent(agentId: AgentId): string {
  return descriptorForAgent(agentId).verifiedCommand
}

export function commandForPreset(preset: AgentPreset, customCommand: string): string {
  if (preset === "custom") return customCommand.trim() || DEFAULT_AGENT_COMMAND
  return commandForAgent(preset)
}

export function agentPresetForCommand(command: string): AgentPreset {
  const normalized = command.trim()
  return AGENT_PRESETS.find((agent) => (
    agent.verifiedCommand === normalized || agent.latestCommand === normalized
  ))?.id ?? "custom"
}

export function resolveCuratedAgentCommand(
  agentId: AgentId,
  commandMode: AgentCommandMode,
  customCommand = "",
): AgentCommandResolution {
  const descriptor = descriptorForAgent(agentId)
  const command = commandMode === "latest"
    ? descriptor.latestCommand
    : commandMode === "custom"
      ? customCommand.trim() || descriptor.verifiedCommand
      : descriptor.verifiedCommand
  return {
    selectedPreset: agentId,
    commandMode,
    command,
    trustedAgentId: commandMode === "custom" ? null : agentId,
  }
}

function descriptorForAgent(agentId: AgentId): AgentDescriptor {
  return AGENT_PRESETS.find((agent) => agent.id === agentId) ?? AGENT_PRESETS[0]
}

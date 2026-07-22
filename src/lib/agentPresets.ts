export type AgentId = "pi" | "claude" | "codex"
export type AgentPreset = AgentId | "custom"
export type AgentCommandMode = "latest" | "custom"

export interface AgentDescriptor {
  id: AgentId
  label: string
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

// Curated ACP adapters always follow the npm latest dist-tag. Custom remains
// available for users who need an explicit wrapper or locally installed agent.
export const AGENT_PRESETS: AgentDescriptor[] = [
  {
    id: "pi",
    label: "Pi",
    latestCommand: "bunx pi-acp@latest",
  },
  {
    id: "claude",
    label: "Claude",
    latestCommand: "bunx @agentclientprotocol/claude-agent-acp@latest",
  },
  {
    id: "codex",
    label: "Codex",
    latestCommand: "bunx @agentclientprotocol/codex-acp@latest",
  },
]

export const DEFAULT_AGENT_ID: AgentId = "pi"
export const DEFAULT_AGENT_COMMAND = AGENT_PRESETS[0].latestCommand

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
  return descriptorForAgent(agentId).latestCommand
}

export function commandForPreset(preset: AgentPreset, customCommand: string): string {
  if (preset === "custom") return customCommand.trim() || DEFAULT_AGENT_COMMAND
  return commandForAgent(preset)
}

export function agentPresetForCommand(command: string): AgentPreset {
  const normalized = command.trim()
  return AGENT_PRESETS.find((agent) => agent.latestCommand === normalized)?.id ?? "custom"
}

// agent_detect_runtimes 的回傳形狀（camelCase，Rust 端 AgentRuntimeAvailability）。
export interface AgentRuntimeAvailability {
  bunx: boolean
  deno: boolean
  node: boolean
  npx: boolean
}

export type RuntimeResolution =
  | { kind: "unchanged"; command: string }
  | { kind: "fallback"; command: string; runtime: "node" }
  | { kind: "unavailable"; command: string }

// #15：curated preset 的 `bunx <pkg>@<ver>` 在 bun 缺席時的 runtime fallback。
// 只改寫 spawn 當下的指令字串，不動 store 裡的設定值（trustedAgentId／custom
// fingerprint 不受影響）。Deno 順位刻意跳過：pi-acp／claude-agent-acp／codex-acp
// 未經 Deno 相容性驗證，規格要求只用經驗證的 invocation——確認相容後再插入分支。
// `npx -y` 對齊 bunx 的非互動語意（否則首次執行會卡在安裝確認提示）。
export function resolveRuntimeCommand(
  command: string,
  runtimes: AgentRuntimeAvailability,
): RuntimeResolution {
  const spec = command.trim()
  if (!spec.startsWith("bunx ")) return { kind: "unchanged", command }
  if (runtimes.bunx) return { kind: "unchanged", command }
  if (runtimes.npx) {
    return {
      kind: "fallback",
      command: `npx -y ${spec.slice("bunx ".length)}`,
      runtime: "node",
    }
  }
  return { kind: "unavailable", command }
}

export function resolveCuratedAgentCommand(
  agentId: AgentId,
  commandMode: AgentCommandMode,
  customCommand = "",
): AgentCommandResolution {
  const descriptor = descriptorForAgent(agentId)
  const command = commandMode === "custom"
    ? customCommand.trim() || descriptor.latestCommand
    : descriptor.latestCommand
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

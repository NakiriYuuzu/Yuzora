export type AgentId = "pi" | "claude" | "codex"
export type AgentPreset = AgentId | "custom"
export type AgentCommandMode = "latest" | "custom"

export interface AgentDescriptor {
  id: AgentId
  label: string
  /**
   * Curated preset 的 spawn 指令。#37 起是**釘選版本**而非 npm latest dist-tag；
   * 欄位名沿用歷史（改名會波及 agentPresetForCommand 的比對與 settingsStorage
   * 的 route 解析），語意以此註解為準。
   */
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

// #37：curated adapter 的釘選版本——release build 用可重現、可稽核的版本，不在
// runtime 解析 npm 的 latest dist-tag（版本漂移會讓未升級的 Yuzora 行為自行改變，且首次下載
// 會被算進 ACP initialize timeout）。升級 adapter＝改這裡＋發版；版本號以
// `bunx npm view <pkg> version` 實測填寫，不可憑印象。既有的「有新版可用」提示
// （agentVersions.ts）不受影響，反而因為釘選才有意義。
export const PINNED_ADAPTER_VERSIONS: Record<AgentId, string> = {
  pi: "0.0.32",
  claude: "0.62.0",
  codex: "1.1.7",
}

const ADAPTER_PACKAGES: Record<AgentId, string> = {
  pi: "pi-acp",
  claude: "@agentclientprotocol/claude-agent-acp",
  codex: "@agentclientprotocol/codex-acp",
}

export function pinnedAdapterCommand(agentId: AgentId): string {
  return `bunx ${ADAPTER_PACKAGES[agentId]}@${PINNED_ADAPTER_VERSIONS[agentId]}`
}

// Curated ACP adapters run the release-pinned version above. Custom remains
// available for users who need an explicit wrapper or locally installed agent.
export const AGENT_PRESETS: AgentDescriptor[] = [
  {
    id: "pi",
    label: "Pi",
    latestCommand: pinnedAdapterCommand("pi"),
  },
  {
    id: "claude",
    label: "Claude",
    latestCommand: pinnedAdapterCommand("claude"),
  },
  {
    id: "codex",
    label: "Codex",
    latestCommand: pinnedAdapterCommand("codex"),
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
  // 偵測到的唯一 JS runtime 是我們還不支援的那個（目前只有 Deno）。與
  // "unavailable" 分開，好讓 prerequisite UI 說得出「你裝了 X，但我們還沒驗證」，
  // 而不是含糊地叫使用者去裝一個他其實已經有的東西。
  | { kind: "unsupported-runtime"; command: string; runtime: "deno" }
  | { kind: "unavailable"; command: string }

// #15：curated preset 的 `bunx <pkg>@<ver>` 在 bun 缺席時的 runtime fallback。
// 只改寫 spawn 當下的指令字串，不動 store 裡的設定值（trustedAgentId／custom
// fingerprint 不受影響）。Deno 順位刻意跳過：pi-acp／claude-agent-acp／codex-acp
// 未經 Deno 相容性驗證，規格要求只用經驗證的 invocation——確認相容後再插入分支；
// 在那之前回 "unsupported-runtime"（顯性、可稽核），而不是硬塞一條沒人驗證過的
// 指令。`npx -y` 對齊 bunx 的非互動語意（否則首次執行會卡在安裝確認提示）。
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
  if (runtimes.deno) return { kind: "unsupported-runtime", command, runtime: "deno" }
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

// Sub-agent 呼叫的跨 adapter 偵測（soak 回饋 2026-07-22：claude/codex/pi 的
// sub-agent 只被當一般 tool block 呈現）。三家 wire 形狀（實地核對版本）：
// - claude-agent-acp（Agent/Task）：title=任務短述、kind:"think"、
//   rawInput={description, prompt, subagent_type, model?}；子 agent 內部 tool
//   call 帶 _meta.claudeCode.parentToolUseId（connection 層已轉 parentToolCallId）。
// - pi（@tintinweb/pi-subagents 的 Agent tool）：rawInput 與 claude 同構
//   （另有 run_in_background?、thinking?）；伴隨 get_subagent_result／
//   steer_subagent 兩個管理 tool（rawInput.agent_id）。
// - codex-acp（collab tool）：title=tool 名（spawn_agent 等）、
//   rawInput={prompt, senderThreadId, receiverThreadIds, agentsStates, status}。
export interface SubagentInvocation {
  kind: "spawn" | "manage" | "collab"
  /** spawn：subagent_type；collab：collab tool 名。 */
  agentType?: string
  /** 任務短述（claude title／rawInput.description）。 */
  task?: string
  /** 完整任務 prompt。 */
  prompt?: string
  model?: string
  background?: boolean
  /** manage（get_subagent_result／steer_subagent）：目標 agent id。 */
  agentId?: string
}

const PI_MANAGE_TOOLS = new Set(["get_subagent_result", "steer_subagent"])

export function subagentInvocation(
  title: string | undefined,
  rawInput: Record<string, unknown> | undefined
): SubagentInvocation | null {
  const input = rawInput ?? {}
  // spawn（claude Agent/Task、pi Agent）：subagent_type＋prompt 是共同指紋。
  if (typeof input.subagent_type === "string" && typeof input.prompt === "string") {
    return {
      kind: "spawn",
      agentType: input.subagent_type,
      ...(typeof input.description === "string" && input.description.trim()
        ? { task: input.description }
        : title
          ? { task: title }
          : {}),
      prompt: input.prompt,
      ...(typeof input.model === "string" && input.model ? { model: input.model } : {}),
      ...(input.run_in_background === true ? { background: true } : {})
    }
  }
  // pi 的管理 tool：以 tool 名（host 翻譯後為 title）識別。
  if (title && PI_MANAGE_TOOLS.has(title) && typeof input.agent_id === "string") {
    return { kind: "manage", agentId: input.agent_id, ...(title ? { task: title } : {}) }
  }
  // codex collab：receiverThreadIds／agentsStates 是 collab item 專屬欄位。
  if (Array.isArray(input.receiverThreadIds) || (input.agentsStates && typeof input.agentsStates === "object")) {
    return {
      kind: "collab",
      ...(title ? { agentType: title, task: title } : {}),
      ...(typeof input.prompt === "string" && input.prompt ? { prompt: input.prompt } : {})
    }
  }
  return null
}

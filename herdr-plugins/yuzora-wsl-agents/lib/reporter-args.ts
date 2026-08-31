import { AGENT, FORBIDDEN_REPORT_TOKENS, SOURCE } from "./constants"

export type AgentState = "working" | "idle" | "blocked" | "unknown"

export function buildReportAgentArgs(input: {
  paneId: string
  state: AgentState
  seq: number
  message?: string
}): string[] {
  const args = [
    "pane",
    "report-agent",
    input.paneId,
    "--source",
    SOURCE,
    "--agent",
    AGENT,
    "--state",
    input.state,
    "--seq",
    String(input.seq)
  ]
  if (input.message) args.push("--message", input.message)
  assertNoSessionIdentity(args)
  return args
}

export function buildReleaseAgentArgs(input: { paneId: string; seq: number }): string[] {
  const args = [
    "pane",
    "release-agent",
    input.paneId,
    "--source",
    SOURCE,
    "--agent",
    AGENT,
    "--seq",
    String(input.seq)
  ]
  assertNoSessionIdentity(args)
  return args
}

export function assertNoSessionIdentity(args: readonly string[]): void {
  const haystack = args.join("\0").toLowerCase()
  for (const token of FORBIDDEN_REPORT_TOKENS) {
    if (haystack.includes(token.toLowerCase())) {
      throw new Error(`forbidden session identity token: ${token}`)
    }
  }
}

export function nextSeq(nowMs: number, persisted: number | null, localBump = 0): number {
  const base = Math.max(nowMs, (persisted ?? 0) + 1)
  return base + localBump
}

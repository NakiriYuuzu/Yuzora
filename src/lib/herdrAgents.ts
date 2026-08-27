import type { HerdrAgentInfo, HerdrAgentStatus } from "./herdrTypes"

const STATUS_URGENCY: Record<HerdrAgentStatus, number> = {
  blocked: 0,
  done: 1,
  working: 2,
  unknown: 3,
  idle: 4
}

/** herdrm-compatible urgency ordering with stable, deterministic tie-breaks. */
export function compareHerdrAgentsByUrgency(
  left: HerdrAgentInfo,
  right: HerdrAgentInfo
): number {
  const status = STATUS_URGENCY[left.status] - STATUS_URGENCY[right.status]
  if (status !== 0) return status

  const leftLabel = left.title ?? left.name
  const rightLabel = right.title ?? right.name
  const label = leftLabel.localeCompare(rightLabel, undefined, {
    numeric: true,
    sensitivity: "base"
  })
  if (label !== 0) return label
  return left.id.localeCompare(right.id)
}

export function sortHerdrAgentsByUrgency(
  agents: readonly HerdrAgentInfo[]
): HerdrAgentInfo[] {
  return [...agents].sort(compareHerdrAgentsByUrgency)
}

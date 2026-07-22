import type { BlockEntry, TranscriptEntry } from "@/agent/acpTypes"

// Chain of Thought 聚合（spec P1）：連續的 tool／thought entries 合併為一個
// activity 群組（渲染成一條可折疊鏈）；被訊息或 perm／plan／diff／error／notice
// 打斷即斷鏈。純函式、不改 transcript 形狀——reducer 與 Session Index replay
// 不受影響。
export type TranscriptSegment =
  | { type: "entry"; entry: TranscriptEntry }
  | { type: "activity"; id: string; entries: BlockEntry[] }

const ACTIVITY_KINDS = new Set<BlockEntry["kind"]>(["tool", "thought"])

export function isActivityEntry(entry: TranscriptEntry): entry is BlockEntry {
  return "kind" in entry && ACTIVITY_KINDS.has(entry.kind)
}

export function segmentTranscript(entries: TranscriptEntry[]): TranscriptSegment[] {
  const segments: TranscriptSegment[] = []
  for (const entry of entries) {
    if (!isActivityEntry(entry)) {
      segments.push({ type: "entry", entry })
      continue
    }
    const last = segments.at(-1)
    if (last?.type === "activity") {
      last.entries.push(entry)
      continue
    }
    // 群組 id 沿用首個 entry 的 stable id：串流中群組尾端增長時 React key 不變。
    segments.push({ type: "activity", id: entry.id, entries: [entry] })
  }
  return segments
}

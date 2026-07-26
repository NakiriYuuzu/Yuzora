import type { LogRecord } from "@/lib/types"

// 沒有 run_id 的歷史 record（issue #40 之前寫下的）在 UI 上的分組代號。
export const UNKNOWN_RUN = "—"

export interface LogRunGroup {
  runId: string
  count: number
  startedAt: string
  endedAt: string
}

/**
 * 依 `run_id` 把載入的 rows 摺成 run 分組（issue #40 AC 第 3 條）。
 *
 * agent／PTY 的 local id 在 restart 後會重用（同一份 daily file 裡會出現兩個
 * `agent-1`），加上 run 分組之後才分得出「這是哪一次執行的 agent-1」。
 *
 * rows 是 newest-first（log_query 的順序），分組結果同樣 newest-first。
 */
export function groupRowsByRun(rows: LogRecord[]): LogRunGroup[] {
  const groups: LogRunGroup[] = []
  const index = new Map<string, LogRunGroup>()
  for (const row of rows) {
    const runId = row.run_id ?? UNKNOWN_RUN
    const existing = index.get(runId)
    if (existing) {
      existing.count += 1
      if (row.timestamp < existing.startedAt) existing.startedAt = row.timestamp
      if (row.timestamp > existing.endedAt) existing.endedAt = row.timestamp
      continue
    }
    const group: LogRunGroup = {
      runId,
      count: 1,
      startedAt: row.timestamp,
      endedAt: row.timestamp,
    }
    index.set(runId, group)
    groups.push(group)
  }
  return groups
}

/**
 * run id 的顯示形式：時間戳前綴對判讀沒有幫助（旁邊就有 timestamp 欄），
 * 隨機尾碼才是用來分辨兩次 run 的部分。
 */
export function shortRunId(runId: string): string {
  const nonce = runId.split("-").at(-1) ?? runId
  return nonce === runId ? runId : nonce
}

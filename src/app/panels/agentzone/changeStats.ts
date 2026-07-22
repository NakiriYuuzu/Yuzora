import type { TranscriptEntry } from "@/agent/acpTypes"

export interface FileChangeStat {
    path: string
    added: number
    removed: number
}

export interface ChangeStats {
    files: FileChangeStat[]
    added: number
    removed: number
}

// 彙總 transcript 中所有 tool meta.diffs（連線層在 diff content 上算好的行數
// 統計）→ 每檔累計＋總計，供 composer 上方的變更統計列使用。無變更回傳 null，
// 統計列因此不渲染。
export function collectChangeStats(transcript: TranscriptEntry[]): ChangeStats | null {
    const byPath = new Map<string, FileChangeStat>()
    for (const entry of transcript) {
        if (!("kind" in entry) || entry.kind !== "tool" || !entry.meta) continue
        for (const diff of metaDiffs(entry.meta)) {
            const current = byPath.get(diff.path) ?? { path: diff.path, added: 0, removed: 0 }
            current.added += diff.added
            current.removed += diff.removed
            byPath.set(diff.path, current)
        }
    }
    if (byPath.size === 0) return null
    const files = [...byPath.values()]
    return {
        files,
        added: files.reduce((sum, file) => sum + file.added, 0),
        removed: files.reduce((sum, file) => sum + file.removed, 0)
    }
}

function metaDiffs(meta: string): FileChangeStat[] {
    try {
        const parsed = JSON.parse(meta) as { diffs?: unknown }
        if (!Array.isArray(parsed.diffs)) return []
        return parsed.diffs.flatMap((item) => {
            if (!item || typeof item !== "object") return []
            const record = item as Record<string, unknown>
            return typeof record.path === "string"
                && typeof record.added === "number"
                && typeof record.removed === "number"
                ? [{ path: record.path, added: record.added, removed: record.removed }]
                : []
        })
    } catch {
        return []
    }
}

import type { EditorState } from "@codemirror/state"
import { getChunks } from "@codemirror/merge"

export const GIT_DIFF_SPLIT_MIN = 0.25
export const GIT_DIFF_SPLIT_MAX = 0.75
export const GIT_DIFF_SPLIT_DEFAULT = 0.5
export const GIT_DIFF_SPLIT_STEP = 0.02
export const FILE_FILTER_MIN_COUNT = 15

export function clampGitDiffSplitRatio(ratio: number): number {
    if (!Number.isFinite(ratio)) return GIT_DIFF_SPLIT_DEFAULT
    return Math.min(GIT_DIFF_SPLIT_MAX, Math.max(GIT_DIFF_SPLIT_MIN, ratio))
}

export function moveListIndex(current: number, total: number, key: string): number | null {
    if (total <= 0) return null
    if (key === "ArrowDown") return Math.min(total - 1, current + 1)
    if (key === "ArrowUp") return Math.max(0, current - 1)
    if (key === "Home") return 0
    if (key === "End") return total - 1
    return null
}

export function pathMatchesFilter(path: string, query: string): boolean {
    const needle = query.trim().toLowerCase()
    if (!needle) return true
    return path.toLowerCase().includes(needle)
}

export function filterRowsByPath<T extends { path: string }>(rows: readonly T[], query: string): T[] {
    return rows.filter((row) => pathMatchesFilter(row.path, query))
}

/** 0-based index of the chunk under (or just before) the main selection. */
export function currentChunkIndex(state: EditorState): number {
    const info = getChunks(state)
    if (!info || info.chunks.length === 0) return -1
    const head = state.selection.main.head
    const side = info.side ?? "b"
    for (let i = 0; i < info.chunks.length; i++) {
        const chunk = info.chunks[i]
        const from = side === "b" ? chunk.fromB : chunk.fromA
        const to = side === "b" ? chunk.toB : chunk.toA
        if (from <= head && head <= to) return i
    }
    for (let i = info.chunks.length - 1; i >= 0; i--) {
        const to = side === "b" ? info.chunks[i].toB : info.chunks[i].toA
        if (to < head) return i
    }
    return 0
}

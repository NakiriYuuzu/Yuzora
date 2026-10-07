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

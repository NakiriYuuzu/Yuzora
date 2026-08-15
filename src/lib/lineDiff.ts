// Line-based added/deleted counts for a diff header (§2.5 L898-899). We count
// lines rather than characters so the +N/−N labels match a git-style hunk view.
// An LCS over the two line arrays yields the number of common lines; everything
// left over on each side is an add or a delete. Trailing empty line from a final
// "\n" is dropped so "a\n" reads as one line, not two.

export interface DiffCounts {
    added: number
    deleted: number
}

/** Skip the O(n*m) LCS when the DP table would exceed this many cells. */
export const LINE_DIFF_CELL_LIMIT = 200_000

function countLines(text: string): number {
    if (text === "") return 0
    let count = 1
    for (let i = 0; i < text.length; i++) {
        if (text.charCodeAt(i) === 10) count++
    }
    if (text.charCodeAt(text.length - 1) === 10) count--
    return count
}

function toLines(text: string): string[] {
    if (text === "") return []
    const lines = text.split("\n")
    if (lines[lines.length - 1] === "") lines.pop()
    return lines
}

// Length of the longest common subsequence of two line arrays. Classic O(n*m)
// DP over a single rolling row to keep memory linear.
function lcsLength(a: string[], b: string[]): number {
    const n = a.length
    const m = b.length
    if (n === 0 || m === 0) return 0
    let prev = new Array<number>(m + 1).fill(0)
    let curr = new Array<number>(m + 1).fill(0)
    for (let i = 1; i <= n; i++) {
        for (let j = 1; j <= m; j++) {
            curr[j] =
                a[i - 1] === b[j - 1]
                    ? prev[j - 1] + 1
                    : Math.max(prev[j], curr[j - 1])
        }
        ;[prev, curr] = [curr, prev]
    }
    return prev[m]
}

function exceedsCellLimit(countA: number, countB: number): boolean {
    if (countA === 0 || countB === 0) return false
    return countA > Math.floor(LINE_DIFF_CELL_LIMIT / countB)
}

export function lineDiffCounts(original: string, modified: string): DiffCounts | null {
    const countA = countLines(original)
    const countB = countLines(modified)
    if (countA === 0) return { added: countB, deleted: 0 }
    if (countB === 0) return { added: 0, deleted: countA }
    if (exceedsCellLimit(countA, countB)) return null
    const a = toLines(original)
    const b = toLines(modified)
    const common = lcsLength(a, b)
    return { added: b.length - common, deleted: a.length - common }
}

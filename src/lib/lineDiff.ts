// Line-based added/deleted counts for a diff header (§2.5 L898-899). We count
// lines rather than characters so the +N/−N labels match a git-style hunk view.
// An LCS over the two line arrays yields the number of common lines; everything
// left over on each side is an add or a delete. Trailing empty line from a final
// "\n" is dropped so "a\n" reads as one line, not two.

export interface DiffCounts {
    added: number
    deleted: number
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

export function lineDiffCounts(original: string, modified: string): DiffCounts {
    const a = toLines(original)
    const b = toLines(modified)
    const common = lcsLength(a, b)
    return { added: b.length - common, deleted: a.length - common }
}

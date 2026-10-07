import { diff } from "@codemirror/merge"

/**
 * Three-way merge model for the conflict merge tool (diff3 over lines).
 *
 * A region is a stretch of the common base that at least one side changed:
 * - `ours` / `theirs`: only that side changed it (non-conflicting),
 * - `both`: both sides made the identical change (non-conflicting),
 * - `conflict`: the sides changed it differently.
 * Ranges are half-open token indexes into the base / ours / theirs arrays.
 */
export type MergeRegionKind = "ours" | "theirs" | "both" | "conflict"
export interface MergeRegion {
    kind: MergeRegionKind
    base: [number, number]
    ours: [number, number]
    theirs: [number, number]
}

interface Hunk {
    aFrom: number
    aTo: number
    bFrom: number
    bTo: number
}

/** Lines including their terminator, so joining them restores the text exactly. */
export function splitLines(text: string): string[] {
    return text.match(/[^\n]*\n|[^\n]+$/g) ?? []
}

const FIRST_TOKEN_CODE = 0x100

// Encodes each distinct token as one UTF-16 code unit so the character diff
// of @codemirror/merge becomes a token diff. Surrogates are skipped.
function encode(tokens: readonly string[], codes: Map<string, number>): string | null {
    let out = ""
    for (const token of tokens) {
        let code = codes.get(token)
        if (code === undefined) {
            code = FIRST_TOKEN_CODE + codes.size
            if (code >= 0xd800) code += 0x800
            if (code > 0xffff) return null
            codes.set(token, code)
        }
        out += String.fromCharCode(code)
    }
    return out
}

function tokenDiff(a: readonly string[], b: readonly string[], codes: Map<string, number>): Hunk[] | null {
    const encodedA = encode(a, codes)
    const encodedB = encode(b, codes)
    if (encodedA === null || encodedB === null) return null
    return diff(encodedA, encodedB).map((change) => ({
        aFrom: change.fromA,
        aTo: change.toA,
        bFrom: change.fromB,
        bTo: change.toB
    }))
}

function sameTokens(a: readonly string[], from: number, to: number, b: readonly string[], bFrom: number, bTo: number): boolean {
    if (to - from !== bTo - bFrom) return false
    for (let i = 0; i < to - from; i++) if (a[from + i] !== b[bFrom + i]) return false
    return true
}

/** diff3 regions between base and two derived token sequences. */
export function mergeRegions(base: readonly string[], ours: readonly string[], theirs: readonly string[]): MergeRegion[] {
    const codes = new Map<string, number>()
    const oursHunks = tokenDiff(base, ours, codes)
    const theirsHunks = tokenDiff(base, theirs, codes)
    if (!oursHunks || !theirsHunks) {
        // Too many distinct tokens to encode: one conflict over everything.
        return [{ kind: "conflict", base: [0, base.length], ours: [0, ours.length], theirs: [0, theirs.length] }]
    }
    const tagged = [
        ...oursHunks.map((hunk) => ({ ...hunk, side: "ours" as const })),
        ...theirsHunks.map((hunk) => ({ ...hunk, side: "theirs" as const }))
    ].sort((a, b) => a.aFrom - b.aFrom || a.aTo - b.aTo)

    const regions: MergeRegion[] = []
    let oursDelta = 0
    let theirsDelta = 0
    let index = 0
    while (index < tagged.length) {
        // A cluster is a run of hunks whose base ranges overlap or touch; like
        // Git, adjacent edits from both sides count as one conflict.
        let lo = tagged[index].aFrom
        let hi = tagged[index].aTo
        const cluster = [tagged[index++]]
        while (index < tagged.length && tagged[index].aFrom <= hi) {
            hi = Math.max(hi, tagged[index].aTo)
            lo = Math.min(lo, tagged[index].aFrom)
            cluster.push(tagged[index++])
        }
        const oursIn = cluster.filter((hunk) => hunk.side === "ours")
        const theirsIn = cluster.filter((hunk) => hunk.side === "theirs")
        const delta = (hunks: Hunk[]) => hunks.reduce((sum, hunk) => sum + (hunk.bTo - hunk.bFrom) - (hunk.aTo - hunk.aFrom), 0)
        const oursRange: [number, number] = [lo + oursDelta, hi + oursDelta + delta(oursIn)]
        const theirsRange: [number, number] = [lo + theirsDelta, hi + theirsDelta + delta(theirsIn)]
        oursDelta += delta(oursIn)
        theirsDelta += delta(theirsIn)
        const kind: MergeRegionKind = !theirsIn.length
            ? "ours"
            : !oursIn.length
                ? "theirs"
                : sameTokens(ours, oursRange[0], oursRange[1], theirs, theirsRange[0], theirsRange[1])
                    ? "both"
                    : "conflict"
        regions.push({ kind, base: [lo, hi], ours: oursRange, theirs: theirsRange })
    }
    return regions
}

/**
 * JetBrains "Resolve simple conflicts": a conflicting region merges cleanly
 * when the two sides' character-level edits do not touch each other.
 * Returns the merged text, or null when the edits really conflict.
 */
export function resolveSimpleConflict(base: string, ours: string, theirs: string): string | null {
    const baseChars = [...base]
    const oursChars = [...ours]
    const theirsChars = [...theirs]
    const regions = mergeRegions(baseChars, oursChars, theirsChars)
    if (regions.some((region) => region.kind === "conflict")) return null
    let out = ""
    let at = 0
    for (const region of regions) {
        out += baseChars.slice(at, region.base[0]).join("")
        out += region.kind === "theirs"
            ? theirsChars.slice(region.theirs[0], region.theirs[1]).join("")
            : oursChars.slice(region.ours[0], region.ours[1]).join("")
        at = region.base[1]
    }
    return out + baseChars.slice(at).join("")
}

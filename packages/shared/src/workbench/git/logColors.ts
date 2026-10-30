// Shared colour helpers for the Log tab.

// §2 graph lane / node palette (blue / green / orange). GraphRow.colorIdx is
// taken modulo this length by the renderer.
export const LANE_COLORS = ["#3b6fe0", "#2bbf8a", "#e08a3b"]

// Author identity colour palette. Real author names are arbitrary (the design's
// static name→colour map can't be used), so we hash the name into a fixed small
// palette — same name always yields the same colour for the row dot + avatar.
const AUTHOR_COLORS = [
    "#3b6fe0",
    "#2bbf8a",
    "#e08a3b",
    "#7b5bff",
    "#c2293f",
    "#178a63",
    "#9a6512",
    "#2456cc"
]

export function authorColor(name: string): string {
    let hash = 0
    for (let i = 0; i < name.length; i++) {
        hash = (hash * 31 + name.charCodeAt(i)) | 0
    }
    return AUTHOR_COLORS[Math.abs(hash) % AUTHOR_COLORS.length]
}

// Initials for the details avatar — first letter of the first two whitespace-
// separated words, uppercased (e.g. "Sora Ito" → "SI", "kenji" → "K").
export function authorInitials(name: string): string {
    const words = name.trim().split(/\s+/).filter(Boolean)
    if (words.length === 0) return "?"
    if (words.length === 1) return words[0].slice(0, 1).toUpperCase()
    return (words[0][0] + words[1][0]).toUpperCase()
}

import type { WorktreeDiffFile } from "@/state/diffModalStore"
import type { GitStatus } from "@/lib/types"

// Shared git file-row primitives for the two change surfaces — the sidebar Git
// nav (E1) and the Local changes tab (E3). Only the badge + worktree-flatten
// helper are shared; the row bodies differ (the sidebar
// row opens the Diff modal and carries stage/unstage; the local row selects into
// the split view and carries discard), so each surface keeps its own FileRow
// rather than forcing an ugly parameterised abstraction (T7 brief §共用元件抽取).

// §5 gitBadge colours (dc.html L3207-3208). Untracked shows "?" and conflicted
// "!" (git-convention chars; §5 has no dedicated entry for either).
const BADGE_COLORS: Record<string, { fg: string; bg: string }> = {
    M: { fg: "#2456cc", bg: "var(--blue-soft)" },
    A: { fg: "#178a63", bg: "var(--mint-soft)" },
    D: { fg: "#c2293f", bg: "var(--danger-soft)" },
    R: { fg: "#9a6512", bg: "var(--amber-soft)" },
    "?": { fg: "#6b6760", bg: "var(--paper-3)" },
    "!": { fg: "#c2293f", bg: "var(--danger-soft)" },
    U: { fg: "#6b6760", bg: "var(--paper-3)" }
}

// §5: normalise a raw git status letter to a single badge char. Unknown → "U".
export function badgeChar(status: string): string {
    const c = status.charAt(0).toUpperCase()
    return c in BADGE_COLORS ? c : "U"
}

// §5 gitBadge — 18×18 r6 mono 10px 700 (dc.html L3210).
export function GitBadge({ badge }: { badge: string }) {
    const { fg, bg } = BADGE_COLORS[badge] ?? BADGE_COLORS.U
    return (
        <span
            aria-hidden="true"
            className="flex size-[18px] shrink-0 items-center justify-center rounded-[6px] font-mono text-[10px] font-bold"
            style={{ background: bg, color: fg }}
        >
            {badge}
        </span>
    )
}

// Flatten the four GitStatus buckets into the modal's WorktreeDiffFile list,
// staged first then working-tree changes — matching the design's openDiffWork
// ordering (dc.html L3215). Untracked → "?", conflicted → "!". Shared by the
// sidebar Review-diff button, the sidebar row click, and contextMenuStore's
// Compare-with-HEAD so all three feed the Diff modal the same list.
export function worktreeFilesFrom(status: GitStatus | null): WorktreeDiffFile[] {
    if (!status) return []
    return [
        ...status.staged.map((e) => ({ path: e.path, origPath: e.origPath, status: e.status, staged: true })),
        ...status.unstaged.map((e) => ({ path: e.path, origPath: e.origPath, status: e.status, staged: false })),
        ...status.untracked.map((path) => ({ path, origPath: null, status: "?", staged: false })),
        ...status.conflicted.map((e) => ({ path: e.path, origPath: e.origPath, status: "!", staged: false }))
    ]
}

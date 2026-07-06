import { useEffect, useRef, useState } from "react"
import { Dialog as DialogPrimitive } from "radix-ui"

import type { DiffContent } from "@/lib/types"
import {
    diffStats,
    langLabel,
    loadCommitDiff,
    loadWorktreeDiff,
    splitPath
} from "@/workbench/git/diffLoad"
import {
    useDiffModalStore,
    type DiffMode,
    type DiffModalSource
} from "@/state/diffModalStore"
import { DiffView } from "@/workbench/git/DiffView"

// §5 gitBadge palette (design L3206-3210) — reused for the file-list rows.
const BADGE_COLORS: Record<string, { fg: string; bg: string }> = {
    M: { fg: "#2456cc", bg: "var(--blue-soft)" },
    A: { fg: "#178a63", bg: "var(--mint-soft)" },
    D: { fg: "#c2293f", bg: "var(--danger-soft)" },
    R: { fg: "#9a6512", bg: "var(--amber-soft)" },
    C: { fg: "#9a6512", bg: "var(--amber-soft)" },
    "?": { fg: "#6b6760", bg: "var(--paper-3)" },
    "!": { fg: "#c2293f", bg: "var(--danger-soft)" },
    U: { fg: "#6b6760", bg: "var(--paper-3)" }
}

function badgeChar(status: string): string {
    const c = status.charAt(0).toUpperCase()
    return c in BADGE_COLORS ? c : "M"
}

function FileBadge({ badge }: { badge: string }) {
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

// The header title/sub differ by source: worktree → "Working tree" + file count;
// commit → shortHash + subject; text → caller-provided title.
function sourceHeader(source: DiffModalSource): { title: string; sub: string } {
    if (source.type === "worktree") {
        const n = source.files.length
        return { title: "Working tree", sub: `${n} changed ${n === 1 ? "file" : "files"}` }
    }
    if (source.type === "text") {
        return { title: source.title, sub: "Agent diff" }
    }
    return { title: source.shortHash, sub: source.subject }
}

// One row's display data, normalised across the two source shapes. `cacheKey`
// disambiguates the per-open diff cache: a worktree MM (partially-staged) file
// appears twice with the same path but different sides, so its key carries the
// side (s/c) to avoid a second row serving the first's cached (wrong-side) diff.
interface Row {
    path: string
    badge: string
    cacheKey: string
}

function sourceRows(source: DiffModalSource): Row[] {
    if (source.type === "worktree") {
        return source.files.map((f) => ({
            path: f.path,
            badge: badgeChar(f.status),
            cacheKey: `${f.staged ? "s" : "c"}:${f.path}`
        }))
    }
    if (source.type === "text") {
        return [{ path: source.title, badge: "M", cacheKey: `text:${source.title}` }]
    }
    // Commit files have a single side per path — key by path (unchanged).
    return source.files.map((f) => ({ path: f.path, badge: badgeChar(f.status), cacheKey: f.path }))
}

// Load the diff for the file at `index`, keyed by path in a per-open cache. The
// commit/worktree branch is chosen from the source; a stale response (activeIndex
// moved on before it resolved) is dropped by the caller via the token check.
function loadDiffFor(source: DiffModalSource, index: number): Promise<DiffContent> {
    if (source.type === "worktree") {
        const f = source.files[index]
        return loadWorktreeDiff(f.path, f.staged)
    }
    if (source.type === "text") {
        return Promise.resolve({ original: source.original, modified: source.modified })
    }
    const f = source.files[index]
    return loadCommitDiff(source.hash, source.parents, f)
}

/**
 * §D (design L1393-1465) Diff viewer modal — app-level, mounted in AppShell.
 * Header (title/sub + Unified/Split toggle + close), a left file list, and the
 * right diff pane which reuses DiffView. Worktree and commit sources load their
 * text through diffLoad. Built on the shadcn Dialog for Esc / focus-trap / a11y,
 * with the design's own overlay + panel styling.
 */
export function DiffModal() {
    const open = useDiffModalStore((s) => s.open)
    const source = useDiffModalStore((s) => s.source)
    const activeIndex = useDiffModalStore((s) => s.activeIndex)
    const mode = useDiffModalStore((s) => s.mode)
    const setActive = useDiffModalStore((s) => s.setActive)
    const setMode = useDiffModalStore((s) => s.setMode)
    const close = useDiffModalStore((s) => s.close)

    const [diff, setDiff] = useState<DiffContent | null>(null)
    // Per-open cache path→loaded diff. Cleared whenever the source identity
    // changes (new open). A ref so mutating it doesn't re-render.
    const cache = useRef<Map<string, DiffContent>>(new Map())
    const cacheKey = useRef<DiffModalSource | null>(null)

    if (cacheKey.current !== source) {
        cache.current = new Map()
        cacheKey.current = source
    }

    const rows = source ? sourceRows(source) : []
    const activeRow = rows[activeIndex] ?? null

    // Load (or serve from cache) the active file's diff. Stale responses are
    // dropped when the active row changed before the load resolved.
    useEffect(() => {
        if (!source || !activeRow) {
            setDiff(null)
            return
        }
        const key = activeRow.cacheKey
        const cached = cache.current.get(key)
        if (cached) {
            setDiff(cached)
            return
        }
        setDiff(null)
        let cancelled = false
        void loadDiffFor(source, activeIndex).then((content) => {
            cache.current.set(key, content)
            if (!cancelled) setDiff(content)
        })
        return () => {
            cancelled = true
        }
        // activeRow.cacheKey identifies the file+side; source identity gates the
        // cache (per-open). eslint-disable-next-line react-hooks/exhaustive-deps
    }, [source, activeIndex, activeRow?.cacheKey])

    if (!open || !source) return null

    const { title, sub } = sourceHeader(source)
    const stats = diff ? diffStats(diff) : null
    const activePath = activeRow?.path ?? ""
    const { name: activeName, dir: activeDir } = activePath
        ? splitPath(activePath)
        : { name: "", dir: "" }
    const lang = activePath ? langLabel(activePath) : ""

    return (
        <DialogPrimitive.Root open={open} onOpenChange={(v) => !v && close()}>
            {/* No Portal: render inside AppShell's relative root so the overlay's
                `absolute inset-0` covers the app container (design L1395). */}
            <>
                {/* Overlay: design L1395 — absolute cover of the app container,
                    translucent ink + 3px backdrop-blur; click closes (the panel
                    is a radix sibling, not a child, so its clicks never reach
                    here — no stopPropagation needed). */}
                <DialogPrimitive.Overlay
                    onClick={() => close()}
                    className="absolute inset-0 z-[62] flex items-center justify-center bg-[rgba(27,26,23,0.34)] p-[24px] supports-backdrop-filter:backdrop-blur-[3px] data-open:animate-in data-open:fade-in-0 data-closed:animate-out data-closed:fade-out-0"
                />
                {/* Panel: design L1396 — 1040px / 88vh paper card. */}
                <DialogPrimitive.Content
                    aria-label={`Diff · ${title}`}
                    onOpenAutoFocus={(e) => e.preventDefault()}
                    className="yz-diffin absolute top-1/2 left-1/2 z-[62] flex h-[88vh] w-[1040px] max-w-[96vw] -translate-x-1/2 -translate-y-1/2 flex-col overflow-hidden rounded-(--r-lg) border border-(--line-2) bg-(--paper-0) shadow-(--shadow-xl) outline-none"
                >
                    <DialogPrimitive.Title className="sr-only">
                        Diff · {title}
                    </DialogPrimitive.Title>
                    <DialogPrimitive.Description className="sr-only">{sub}</DialogPrimitive.Description>

                    {/* header — design L1398 */}
                    <div className="flex h-[52px] shrink-0 items-center gap-[11px] border-b border-(--line-1) bg-(--paper-1) pr-[14px] pl-[17px]">
                        <svg
                            width="17"
                            height="17"
                            viewBox="0 0 24 24"
                            fill="none"
                            stroke="#3b6fe0"
                            strokeWidth="1.9"
                            strokeLinecap="round"
                            strokeLinejoin="round"
                            className="shrink-0"
                            aria-hidden="true"
                        >
                            <path d="M8 3v18M3 8h5M16 21V3M16 16h5" />
                        </svg>
                        <div className="flex min-w-0 flex-col gap-[1px]">
                            <div className="whitespace-nowrap font-serif text-[15px] font-semibold leading-[1.1] text-(--ink-0)">
                                Diff · {title}
                            </div>
                            <div className="truncate font-mono text-[10.5px] text-(--ink-3)">{sub}</div>
                        </div>
                        <div className="flex-1" />
                        {/* §4.2 unified/split toggle */}
                        <div className="flex shrink-0 gap-[3px] rounded-[9px] bg-(--yz-sunk) p-[3px]">
                            {(["unified", "split"] as const).map((m: DiffMode) => (
                                <button
                                    key={m}
                                    type="button"
                                    aria-pressed={mode === m}
                                    onClick={() => setMode(m)}
                                    className={
                                        "h-[26px] rounded-[7px] px-[12px] text-[11px] font-semibold transition-all duration-[140ms] " +
                                        (mode === m
                                            ? "bg-(--yz-solid) text-(--ink-0) shadow-(--shadow-xs)"
                                            : "text-(--ink-3)")
                                    }
                                >
                                    {m === "unified" ? "Unified" : "Split"}
                                </button>
                            ))}
                        </div>
                        <button
                            type="button"
                            aria-label="Close"
                            title="Close"
                            onClick={() => close()}
                            className="flex size-[30px] shrink-0 items-center justify-center rounded-[9px] text-(--ink-3) transition-all duration-150 hover:bg-(--paper-2) hover:text-(--ink-0)"
                        >
                            <svg
                                width="16"
                                height="16"
                                viewBox="0 0 24 24"
                                fill="none"
                                stroke="currentColor"
                                strokeWidth="2"
                                strokeLinecap="round"
                                aria-hidden="true"
                            >
                                <path d="M18 6 6 18M6 6l12 12" />
                            </svg>
                        </button>
                    </div>

                    <div className="flex min-h-0 flex-1">
                        {/* left file list — design L1413 */}
                        <div className="yzs w-[236px] shrink-0 overflow-auto border-r border-(--line-1) bg-(--paper-1) p-[9px]">
                            <div className="px-[8px] pt-[5px] pb-[7px] text-[9.5px] font-semibold tracking-[0.08em] text-(--ink-3) uppercase">
                                {sub}
                            </div>
                            {rows.map((row, i) => {
                                const { name, dir } = splitPath(row.path)
                                const selected = i === activeIndex
                                return (
                                    <button
                                        key={`${row.path}:${i}`}
                                        type="button"
                                        onClick={() => setActive(i)}
                                        className={
                                            "flex h-[32px] w-full items-center gap-[9px] rounded-[8px] px-[8px] my-[1px] text-left transition-[background] duration-[120ms] " +
                                            (selected
                                                ? "bg-(--yz-active) shadow-(--shadow-xs)"
                                                : "hover:bg-(--yz-panel)")
                                        }
                                    >
                                        <FileBadge badge={row.badge} />
                                        <span className="min-w-0 flex-1 truncate">
                                            <span
                                                className={
                                                    "text-[12px] " +
                                                    (selected
                                                        ? "font-semibold text-(--ink-0)"
                                                        : "font-medium text-(--ink-1)")
                                                }
                                            >
                                                {name}
                                            </span>
                                            {dir && (
                                                <span className="ml-[6px] text-[10px] text-(--ink-4)">{dir}</span>
                                            )}
                                        </span>
                                    </button>
                                )
                            })}
                        </div>

                        {/* diff body — design L1423 */}
                        <div className="flex min-w-0 flex-1 flex-col">
                            <div className="flex h-[38px] shrink-0 items-center gap-[9px] border-b border-(--line-1) bg-(--yz-sunk) px-[16px]">
                                <span className="min-w-0 flex-1 truncate font-mono text-[12px] font-semibold text-(--ink-1)">
                                    {activeDir}
                                    {activeName}
                                </span>
                                {lang && (
                                    <span className="shrink-0 font-mono text-[11px] text-(--ink-3)">{lang}</span>
                                )}
                                {stats && (
                                    <>
                                        <span
                                            className="shrink-0 font-mono text-[11px] font-semibold"
                                            style={{ color: "#178a63" }}
                                        >
                                            +{stats.added}
                                        </span>
                                        <span
                                            className="shrink-0 font-mono text-[11px] font-semibold"
                                            style={{ color: "#c2293f" }}
                                        >
                                            −{stats.deleted}
                                        </span>
                                    </>
                                )}
                            </div>
                            <div className="min-h-0 flex-1 overflow-hidden bg-(--paper-0)">
                                {diff ? (
                                    <DiffView content={diff} mode={mode} />
                                ) : (
                                    <div className="flex h-full items-center justify-center text-[12.5px] text-(--ink-3)">
                                        Loading diff…
                                    </div>
                                )}
                            </div>
                        </div>
                    </div>
                </DialogPrimitive.Content>
            </>
        </DialogPrimitive.Root>
    )
}

import { useMemo, useRef, useState } from "react"

import type { LogCommit, LogRef } from "@/lib/types"
import { computeGraphLayout } from "@/workbench/git/graphLayout"
import type { GraphRow } from "@/workbench/git/graphLayout"
import { relativeTime } from "@/lib/relativeTime"
import { authorColor, LANE_COLORS } from "@/workbench/git/logColors"

// §2 L762-815 geometry.
const ROW_HEIGHT = 32
const GRAPH_WIDTH = 72
const LANE_X0 = 18 // lane 0 centre (design SVG cx=18)
const LANE_GAP = 16 // lane spacing (cx 18/34/50)
const OVERSCAN = 10 // rows rendered above/below the viewport
// Trigger loadMore once the scroll bottom is within this many px of the end.
const LOAD_MORE_THRESHOLD = 400

function laneX(lane: number): number {
    return LANE_X0 + lane * LANE_GAP
}

// §2 refChip (design L3399-3410). HEAD is merged into the matching local branch
// so we don't show a bare "HEAD" chip next to "main"; a detached HEAD with no
// local branch still surfaces as a solid HEAD pill.
type ChipKind = "head" | "main" | "mainR" | "feat" | "fix" | "tag"

const CHIP_STYLES: Record<ChipKind, { bg: string; color: string }> = {
    head: { bg: "var(--ink-1)", color: "var(--paper-0)" },
    main: { bg: "var(--blue-soft)", color: "#2456cc" },
    mainR: { bg: "rgba(59,111,224,0.10)", color: "#5a7fd0" },
    feat: { bg: "var(--mint-soft)", color: "#0f7a55" },
    fix: { bg: "rgba(224,138,59,0.16)", color: "#9a5512" },
    tag: { bg: "var(--amber-soft)", color: "#9a6512" }
}

interface Chip {
    label: string
    kind: ChipKind
}

// Map the T2 ref set to design chips. `head` refs are folded into the local
// branch they point at (§brief: avoid duplicate HEAD+main). A remote branch uses
// the softer `mainR` blue; local branches use `main`; tags use `tag`. feat/fix
// are name-heuristic accents so branch chips echo the prototype's colour hints.
function refsToChips(refs: LogRef[]): Chip[] {
    const hasHead = refs.some((r) => r.kind === "head")
    const localNames = new Set(refs.filter((r) => r.kind === "local").map((r) => r.name))

    const chips: Chip[] = []
    // A detached HEAD (HEAD ref present but no local branch here) shows a solid
    // HEAD pill; otherwise HEAD is implied by the highlighted local branch chip.
    if (hasHead && localNames.size === 0) {
        chips.push({ label: "HEAD", kind: "head" })
    }
    for (const ref of refs) {
        if (ref.kind === "head") continue
        if (ref.kind === "tag") {
            chips.push({ label: ref.name, kind: "tag" })
        } else if (ref.kind === "remote") {
            chips.push({ label: ref.name, kind: "mainR" })
        } else {
            // local branch: pick an accent from the name for a bit of the
            // prototype's colour variety (feat/fix), default to the blue chip.
            const kind: ChipKind = /feat/i.test(ref.name)
                ? "feat"
                : /fix|hotfix|bug/i.test(ref.name)
                  ? "fix"
                  : "main"
            chips.push({ label: ref.name, kind })
        }
    }
    return chips
}

function RefChip({ chip }: { chip: Chip }) {
    const s = CHIP_STYLES[chip.kind]
    return (
        <span
            className="inline-flex h-[17px] shrink-0 items-center gap-[3px] whitespace-nowrap rounded-[5px] px-[6px] font-mono text-[9.5px] font-semibold"
            style={{ background: s.bg, color: s.color }}
        >
            {chip.label}
        </span>
    )
}

// A row projected for rendering: the layout row plus the `refs` count so the
// graph node knows whether to draw the larger ref ring.
interface LayoutRow extends GraphRow {
    refs: number
}

// §2 L778-797 — the SVG graph for the currently-windowed rows only. Only the
// visible slice of nodes/segments is drawn inside the tall spacer at absolute
// positions derived from each row's index.
function GraphSvg({
    rows,
    startIndex,
    endIndex,
    totalHeight
}: {
    rows: LayoutRow[]
    startIndex: number
    endIndex: number
    totalHeight: number
}) {
    return (
        <svg
            width={GRAPH_WIDTH}
            height={totalHeight}
            fill="none"
            className="pointer-events-none absolute left-0 top-0"
            aria-hidden="true"
        >
            {rows.slice(startIndex, endIndex).map((row, i) => {
                const index = startIndex + i
                const rowTop = index * ROW_HEIGHT
                const cy = rowTop + ROW_HEIGHT / 2
                const nodeColor = LANE_COLORS[row.colorIdx % LANE_COLORS.length]
                const hasRefs = row.refs > 0
                return (
                    <g key={row.hash}>
                        {row.segments.map((seg, si) => {
                            const x1 = laneX(seg.fromLane)
                            const x2 = laneX(seg.toLane)
                            const yTop = rowTop
                            const yBot = rowTop + ROW_HEIGHT
                            const color = LANE_COLORS[seg.colorIdx % LANE_COLORS.length]
                            const d =
                                seg.fromLane === seg.toLane
                                    ? `M${x1} ${yTop} L${x2} ${yBot}`
                                    : // Cubic curve mirroring the design's branch-out /
                                      // merge-in shape (control points at the row midline).
                                      `M${x1} ${yTop} C${x1} ${cy} ${x2} ${cy} ${x2} ${yBot}`
                            return (
                                <path
                                    key={si}
                                    d={d}
                                    stroke={color}
                                    strokeWidth={2}
                                    strokeLinecap="round"
                                />
                            )
                        })}
                        {/* node — refs get a larger ring + inner dot (§2 L785/789) */}
                        <circle
                            cx={laneX(row.lane)}
                            cy={cy}
                            r={hasRefs ? 5.2 : 4.5}
                            fill={nodeColor}
                            stroke="var(--yz-node-ring)"
                            strokeWidth={2}
                        />
                        {hasRefs && (
                            <circle
                                cx={laneX(row.lane)}
                                cy={cy}
                                r={1.8}
                                fill="var(--yz-node-ring)"
                            />
                        )}
                    </g>
                )
            })}
        </svg>
    )
}

function CommitRow({
    commit,
    top,
    selected,
    onSelect
}: {
    commit: LogCommit
    top: number
    selected: boolean
    onSelect: () => void
}) {
    const chips = refsToChips(commit.refs)
    return (
        <div
            role="button"
            tabIndex={0}
            aria-pressed={selected}
            onClick={onSelect}
            className="absolute left-0 flex h-[32px] w-full cursor-pointer items-stretch"
            style={{ top }}
        >
            <span className="w-[72px] shrink-0" aria-hidden="true" />
            <span
                className={
                    "flex h-[32px] min-w-0 flex-1 items-center gap-[8px] px-[12px] " +
                    (selected
                        ? "bg-(--yz-active) shadow-[inset_2px_0_0_#3b6fe0]"
                        : "hover:bg-(--yz-panel)")
                }
            >
                <span className="flex min-w-0 flex-1 items-center gap-[6px] overflow-hidden">
                    {chips.map((chip, i) => (
                        <RefChip key={i} chip={chip} />
                    ))}
                    <span className="truncate text-[12.5px] text-(--ink-1)">
                        {commit.subject}
                    </span>
                </span>
                <span className="flex w-[64px] shrink-0 items-center gap-[6px] overflow-hidden text-[11.5px] text-(--ink-2)">
                    <span
                        aria-hidden="true"
                        className="size-[7px] shrink-0 rounded-full"
                        style={{ background: authorColor(commit.authorName) }}
                    />
                    <span className="truncate">{commit.authorName}</span>
                </span>
                <span className="w-[34px] shrink-0 text-right font-mono text-[10.5px] text-(--ink-3)">
                    {relativeTime(commit.timestamp)}
                </span>
            </span>
        </div>
    )
}

/**
 * Virtualized graph + commit list (§2 L762-815). Owns its own scroll container
 * so windowing math (scrollTop → visible index range) and infinite-scroll live
 * together. The graph is laid out from the full commit list (cheap, pure) but
 * only the windowed slice of nodes/segments and rows is rendered.
 */
export function LogGraph({
    commits,
    selectedHash,
    onSelect,
    hasMore,
    loadingMore,
    onLoadMore
}: {
    commits: LogCommit[]
    selectedHash: string | null
    onSelect: (hash: string) => void
    hasMore: boolean
    loadingMore: boolean
    onLoadMore: () => void
}) {
    const scrollRef = useRef<HTMLDivElement | null>(null)
    const [scrollTop, setScrollTop] = useState(0)
    const [viewportHeight, setViewportHeight] = useState(0)

    // Lane layout over the whole loaded list. Pure + memoised on the commit
    // identity list so it only recomputes when commits actually change.
    const layout = useMemo(
        () => computeGraphLayout(commits.map((c) => ({ hash: c.hash, parents: c.parents }))),
        [commits]
    )
    const rows: LayoutRow[] = useMemo(
        () =>
            layout.rows.map((r, i) => ({
                ...r,
                refs: commits[i]?.refs.length ?? 0
            })),
        [layout, commits]
    )

    const total = commits.length
    const totalHeight = total * ROW_HEIGHT

    // Visible window: clamp [scrollTop, scrollTop+viewport] to row indices, pad
    // with overscan. viewportHeight starts at 0 (pre-measure) so we fall back to
    // rendering a reasonable first screen from the top.
    const effectiveViewport = viewportHeight || 600
    const startIndex = Math.max(0, Math.floor(scrollTop / ROW_HEIGHT) - OVERSCAN)
    const endIndex = Math.min(
        total,
        Math.ceil((scrollTop + effectiveViewport) / ROW_HEIGHT) + OVERSCAN
    )

    function onScroll(e: React.UIEvent<HTMLDivElement>) {
        const el = e.currentTarget
        setScrollTop(el.scrollTop)
        setViewportHeight(el.clientHeight)
        // Infinite scroll: near the bottom, pull the next page.
        if (
            hasMore &&
            !loadingMore &&
            el.scrollHeight - el.scrollTop - el.clientHeight < LOAD_MORE_THRESHOLD
        ) {
            onLoadMore()
        }
    }

    return (
        <div className="flex min-h-0 flex-1 flex-col border-r border-(--line-1)">
            {/* §2 L768-775 header row */}
            <div className="flex h-[28px] shrink-0 items-center border-b border-(--line-1) bg-(--paper-1) font-sans text-[9.5px] font-bold uppercase tracking-[0.07em] text-(--ink-3)">
                <span className="w-[72px] shrink-0" aria-hidden="true" />
                <span className="flex min-w-0 flex-1 items-center gap-[8px] px-[12px]">
                    <span className="flex-1">Commit</span>
                    <span className="w-[64px] shrink-0">Author</span>
                    <span className="w-[34px] shrink-0 text-right">Date</span>
                </span>
            </div>

            <div
                ref={scrollRef}
                data-testid="log-scroll"
                onScroll={onScroll}
                className="yzs min-h-0 flex-1 overflow-auto"
            >
                <div className="relative" style={{ height: totalHeight, minHeight: totalHeight }}>
                    <GraphSvg
                        rows={rows}
                        startIndex={startIndex}
                        endIndex={endIndex}
                        totalHeight={totalHeight}
                    />
                    {commits.slice(startIndex, endIndex).map((commit, i) => {
                        const index = startIndex + i
                        return (
                            <CommitRow
                                key={commit.hash}
                                commit={commit}
                                top={index * ROW_HEIGHT}
                                selected={commit.hash === selectedHash}
                                onSelect={() => onSelect(commit.hash)}
                            />
                        )
                    })}
                </div>
            </div>
        </div>
    )
}

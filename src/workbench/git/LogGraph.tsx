import { memo, useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState } from "react"
import { ScrollArea } from "@/components/ui/scroll-area"
import { useTranslation } from "react-i18next"

import { Button } from "@/components/ui/button"

import type { LogCommit, LogRef } from "@/lib/types"
import { computeGraphLayout } from "@/workbench/git/graphLayout"
import type { GraphRow } from "@/workbench/git/graphLayout"
import { relativeTime } from "@/lib/relativeTime"
import { authorColor, LANE_COLORS } from "@/workbench/git/logColors"

// §2 L762-815 geometry.
const ROW_HEIGHT = 32
const MIN_GRAPH_WIDTH = 72
const HEADER_HEIGHT = 28
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
const GraphSvg = memo(function GraphSvg({
    rows,
    startIndex,
    endIndex,
    graphWidth
}: {
    rows: LayoutRow[]
    startIndex: number
    endIndex: number
    graphWidth: number
}) {
    const windowHeight = Math.max(0, (endIndex - startIndex) * ROW_HEIGHT)
    return (
        <svg
            width={graphWidth}
            height={windowHeight}
            fill="none"
            data-testid="log-graph-svg"
            className="pointer-events-none absolute left-0"
            style={{ top: startIndex * ROW_HEIGHT }}
            aria-hidden="true"
        >
            {rows.slice(startIndex, endIndex).map((row, i) => {
                const rowTop = i * ROW_HEIGHT
                const cy = rowTop + ROW_HEIGHT / 2
                const nodeColor = LANE_COLORS[row.colorIdx % LANE_COLORS.length]
                const hasRefs = row.refs > 0
                return (
                    <g key={row.hash}>
                        {row.segments.map((seg, si) => {
                            const x1 = laneX(seg.fromLane)
                            const x2 = laneX(seg.toLane)
                            const yTop = seg.phase === "outgoing" ? cy : rowTop
                            const yBot = seg.phase === "incoming" ? cy : rowTop + ROW_HEIGHT
                            const curveY = (yTop + yBot) / 2
                            const color = LANE_COLORS[seg.colorIdx % LANE_COLORS.length]
                            const d =
                                seg.fromLane === seg.toLane
                                    ? `M${x1} ${yTop} L${x2} ${yBot}`
                                    : // Branch/merge half-row curves connect at the node centre.
                                      `M${x1} ${yTop} C${x1} ${curveY} ${x2} ${curveY} ${x2} ${yBot}`
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
})

const CommitRow = memo(function CommitRow({
    commit,
    top,
    selected,
    onSelect,
    columns
}: {
    commit: LogCommit
    top: number
    selected: boolean
    onSelect: (hash: string) => void
    columns: string
}) {
    const chips = refsToChips(commit.refs)
    return (
        <div
            role="button"
            tabIndex={0}
            aria-pressed={selected}
            onClick={() => onSelect(commit.hash)}
            onKeyDown={(event) => {
                if (event.key === "Enter" || event.key === " ") {
                    event.preventDefault()
                    onSelect(commit.hash)
                }
            }}
            className="absolute left-0 grid h-[32px] w-full cursor-pointer items-stretch outline-none focus-visible:ring-2 focus-visible:ring-inset focus-visible:ring-(--yz-accent)"
            style={{ top, gridTemplateColumns: columns }}
        >
            <span aria-hidden="true" />
            <span
                className={
                    "col-span-3 grid h-[32px] grid-cols-subgrid items-center " +
                    (selected
                        ? "bg-(--yz-active) shadow-[inset_2px_0_0_#3b6fe0]"
                        : "hover:bg-(--yz-panel)")
                }
            >
                <span className="flex items-center gap-[6px] whitespace-nowrap px-[12px]">
                    {chips.map((chip, i) => (
                        <RefChip key={i} chip={chip} />
                    ))}
                    <span className="whitespace-nowrap text-[12.5px] text-(--ink-1)">
                        {commit.subject}
                    </span>
                </span>
                <span className="flex items-center gap-[6px] whitespace-nowrap px-[8px] text-[11.5px] text-(--ink-2)">
                    <span
                        aria-hidden="true"
                        className="size-[7px] shrink-0 rounded-full"
                        style={{ background: authorColor(commit.authorName) }}
                    />
                    <span>{commit.authorName}</span>
                </span>
                <span className="whitespace-nowrap px-[12px] text-right font-mono text-[10.5px] text-(--ink-3)">
                    {relativeTime(commit.timestamp)}
                </span>
            </span>
        </div>
    )
})

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
    loadMoreError,
    onLoadMore,
    onRetryLoadMore
}: {
    commits: LogCommit[]
    selectedHash: string | null
    onSelect: (hash: string) => void
    hasMore: boolean
    loadingMore: boolean
    loadMoreError?: string | null
    onLoadMore: () => void
    onRetryLoadMore?: () => void
}) {
    const { t } = useTranslation("menus")
    const scrollRef = useRef<HTMLDivElement | null>(null)
    const lastScrollTop = useRef(0)
    const requestedPage = useRef<number | null>(null)
    const firstHash = commits[0]?.hash
    useEffect(() => { requestedPage.current = null }, [firstHash])
    const rafRef = useRef<number | null>(null)
    const pendingScrollRef = useRef<{ scrollTop: number; viewportHeight: number } | null>(null)
    const onSelectRef = useRef(onSelect)
    useEffect(() => {
        onSelectRef.current = onSelect
    }, [onSelect])
    const selectHash = useCallback((hash: string) => {
        onSelectRef.current(hash)
    }, [])
    const [viewState, setViewState] = useState({
        startIndex: 0,
        endIndex: Math.ceil(600 / ROW_HEIGHT) + OVERSCAN,
        viewportHeight: 0
    })

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

    const graphWidth = Math.max(MIN_GRAPH_WIDTH, laneX(Math.max(0, layout.laneCount - 1)) + 12)
    const [textWidths, setTextWidths] = useState({ subject: 320, author: 100, date: 70 })
    useLayoutEffect(() => {
        let active = true
        const measure = () => {
            if (!active) return
            // jsdom and restricted renderers may not provide a canvas context.
            let context: CanvasRenderingContext2D | null = null
            try {
                if (typeof CanvasRenderingContext2D !== "undefined") context = document.createElement("canvas").getContext("2d")
            } catch { /* Use conservative full-width glyph estimates below. */ }
            const styles = getComputedStyle(scrollRef.current!)
            const sans = styles.getPropertyValue("--font-sans").trim() || "sans-serif"
            const mono = styles.getPropertyValue("--font-mono").trim() || "monospace"
            const width = (text: string, font: string) => {
                if (!context) return [...text].length * 14
                context.font = font
                return context.measureText(text).width
            }
            let subject = 320, author = 100, date = 70
            for (const commit of commits) {
                const chips = refsToChips(commit.refs)
                const chipWidth = chips.reduce((sum, chip) => sum + width(chip.label, `600 9.5px ${mono}`) + 18, 0)
                subject = Math.max(subject, width(commit.subject, `12.5px ${sans}`) + chipWidth + 32)
                author = Math.max(author, width(commit.authorName, `11.5px ${sans}`) + 32)
                date = Math.max(date, width(relativeTime(commit.timestamp), `10.5px ${mono}`) + 28)
            }
            setTextWidths({ subject: Math.ceil(subject), author: Math.ceil(author), date: Math.ceil(date) })
        }
        measure()
        void document.fonts?.ready.then(measure)
        return () => { active = false }
    }, [commits])
    const columns = `${graphWidth}px minmax(${textWidths.subject}px, 1fr) ${textWidths.author}px ${textWidths.date}px`
    const tableWidth = graphWidth + textWidths.subject + textWidths.author + textWidths.date

    const total = commits.length
    const totalHeight = total * ROW_HEIGHT

    function deriveWindow(scrollTop: number, viewportHeight: number, count: number) {
        const effectiveViewport = Math.max(0, (viewportHeight || 600) - HEADER_HEIGHT)
        return {
            startIndex: Math.max(0, Math.floor(scrollTop / ROW_HEIGHT) - OVERSCAN),
            endIndex: Math.min(
                count,
                Math.ceil((scrollTop + effectiveViewport) / ROW_HEIGHT) + OVERSCAN
            ),
            viewportHeight
        }
    }

    function commitWindow(scrollTop: number, viewportHeight: number, count = total) {
        const next = deriveWindow(scrollTop, viewportHeight, count)
        setViewState((prev) => (
            prev.startIndex === next.startIndex
            && prev.endIndex === next.endIndex
            && prev.viewportHeight === next.viewportHeight
                ? prev
                : next
        ))
    }

    useEffect(() => {
        const el = scrollRef.current
        if (!el) return
        const schedule = () => {
            pendingScrollRef.current = { scrollTop: el.scrollTop, viewportHeight: el.clientHeight }
            if (rafRef.current == null) {
                rafRef.current = requestAnimationFrame(() => {
                    rafRef.current = null
                    const pending = pendingScrollRef.current
                    if (!pending) return
                    commitWindow(pending.scrollTop, pending.viewportHeight, commits.length)
                })
            }
        }
        schedule()
        const observer = new ResizeObserver(() => schedule())
        observer.observe(el)
        return () => {
            observer.disconnect()
            if (rafRef.current != null) cancelAnimationFrame(rafRef.current)
            rafRef.current = null
        }
        // Re-bind when the loaded list length changes so append/reload remeasure.
        // eslint-disable-next-line react-hooks/exhaustive-deps
    }, [commits.length])

    const startIndex = Math.min(viewState.startIndex, Math.max(0, total))
    const endIndex = Math.min(total, Math.max(startIndex, viewState.endIndex))

    function onScroll(e: React.UIEvent<HTMLDivElement>) {
        const el = e.currentTarget
        pendingScrollRef.current = { scrollTop: el.scrollTop, viewportHeight: el.clientHeight }
        if (rafRef.current == null) {
            rafRef.current = requestAnimationFrame(() => {
                rafRef.current = null
                const pending = pendingScrollRef.current
                if (!pending) return
                commitWindow(pending.scrollTop, pending.viewportHeight)
            })
        }
        const movedVertically = el.scrollTop !== lastScrollTop.current
        lastScrollTop.current = el.scrollTop
        // A horizontal scroll must neither paginate nor duplicate an in-flight page.
        if (
            movedVertically &&
            requestedPage.current !== total &&
            !loadMoreError &&
            hasMore &&
            !loadingMore &&
            el.scrollHeight - el.scrollTop - el.clientHeight < LOAD_MORE_THRESHOLD
        ) {
            requestedPage.current = total
            onLoadMore()
        }
    }

    return (
        <div className="flex min-h-0 min-w-0 flex-1 flex-col border-r border-(--line-1)">
            <ScrollArea
                className="min-h-0 min-w-0 w-full flex-1"
                orientation="both"
                viewportRef={scrollRef}
                viewportProps={{
                    "data-testid": "log-scroll",
                    onScroll,
                }}
            >
                <div data-testid="log-table" style={{ minWidth: tableWidth, width: "100%" }}>
                    <div data-testid="log-header" className="sticky top-0 z-10 grid h-[28px] items-center border-b border-(--line-1) bg-(--paper-1) font-sans text-[9.5px] font-bold uppercase tracking-[0.07em] text-(--ink-3)" style={{ gridTemplateColumns: columns }}>
                        <span className="px-[12px]">{t("logGraph.branchColumn")}</span>
                        <span className="px-[12px]">{t("logGraph.commitColumn")}</span>
                        <span className="px-[8px]">{t("logGraph.authorColumn")}</span>
                        <span className="px-[12px] text-right">{t("logGraph.dateColumn")}</span>
                    </div>
                <div className="relative" style={{ height: totalHeight, minHeight: totalHeight }}>
                    <GraphSvg
                        graphWidth={graphWidth}
                        rows={rows}
                        startIndex={startIndex}
                        endIndex={endIndex}
                    />
                    {commits.slice(startIndex, endIndex).map((commit, i) => {
                        const index = startIndex + i
                        return (
                            <CommitRow
                                key={commit.hash}
                                commit={commit}
                                columns={columns}
                                top={index * ROW_HEIGHT}
                                selected={commit.hash === selectedHash}
                                onSelect={selectHash}
                            />
                        )
                    })}
                </div>
                </div>
            </ScrollArea>
            {loadMoreError && (
                <div
                    data-testid="log-load-more-error"
                    className="flex shrink-0 items-center gap-[8px] border-t border-(--line-1) px-[12px] py-[7px]"
                >
                    <span role="alert" className="min-w-0 flex-1 truncate text-[11px] text-(--ink-2)">
                        {t("logTab.loadMoreFailed", { message: loadMoreError })}
                    </span>
                    {onRetryLoadMore && (
                        <Button
                            type="button"
                            size="xs"
                            onClick={onRetryLoadMore}
                            className="shrink-0 rounded-[7px] border border-(--line-1) bg-(--yz-solid) px-[9px] py-[3px] text-[11px] font-semibold text-(--ink-1) shadow-(--shadow-xs) hover:bg-(--paper-1)"
                        >
                            {t("logTab.retry")}
                        </Button>
                    )}
                </div>
            )}
        </div>
    )
}

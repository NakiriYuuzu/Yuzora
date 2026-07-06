import { useEffect, useRef, useState } from "react"

import type { CommitFileChange } from "@/lib/types"
import { logUserAction } from "@/features/logs/userAction"
import { gitCheckout, gitCherryPick } from "@/lib/ipc"
import { useGitLogStore } from "@/state/gitLogStore"
import { useGitStore } from "@/state/gitStore"
import { useWorkspaceStore } from "@/state/workspaceStore"
import { LogGraph } from "@/workbench/git/LogGraph"
import { CommitDetails } from "@/workbench/git/CommitDetails"

// Debounce for the search box (§brief C3: 250ms in the UI layer; the store
// reloads immediately when setFilters fires).
const QUERY_DEBOUNCE_MS = 250

// §brief C3 Date dropdown — minimal usable semantics over the design's static
// decoration. Maps a label to filters.since (git-recognised relative dates);
// until stays null. Reported as an explicit interpretation.
const DATE_OPTIONS: { label: string; since: string | null }[] = [
    { label: "All", since: null },
    { label: "Today", since: "1 day ago" },
    { label: "Last 7 days", since: "7 days ago" },
    { label: "Last 30 days", since: "30 days ago" }
]

function ChevronDown() {
    return (
        <svg
            width="12"
            height="12"
            viewBox="0 0 24 24"
            fill="none"
            stroke="var(--ink-3)"
            strokeWidth="2"
            strokeLinecap="round"
            strokeLinejoin="round"
            aria-hidden="true"
        >
            <path d="m6 9 6 6 6-6" />
        </svg>
    )
}

// §2 L750-757 frost-glass filter dropdown. Kept as a lightweight
// absolutely-positioned menu (matching the prototype) rather than a portalled
// Radix menu so it stays trivially testable and self-contained.
function FilterDropdown({
    field,
    value,
    options,
    onSelect
}: {
    field: string
    value: string
    options: { key: string; label: string }[]
    onSelect: (key: string) => void
}) {
    const [open, setOpen] = useState(false)
    const ref = useRef<HTMLDivElement | null>(null)

    useEffect(() => {
        if (!open) return
        function onDown(e: MouseEvent) {
            if (ref.current && !ref.current.contains(e.target as Node)) setOpen(false)
        }
        document.addEventListener("mousedown", onDown)
        return () => document.removeEventListener("mousedown", onDown)
    }, [open])

    return (
        <div className="relative" ref={ref}>
            <button
                type="button"
                aria-label={`${field} filter`}
                aria-haspopup="menu"
                aria-expanded={open}
                onClick={() => setOpen((v) => !v)}
                className="flex h-[28px] cursor-pointer items-center gap-[5px] rounded-[9px] border border-(--line-1) bg-(--yz-solid) px-[10px] text-[11.5px] text-(--ink-1) transition-colors hover:bg-(--paper-1)"
            >
                <span className="text-(--ink-3)">{field}:</span>
                {value}
                <ChevronDown />
            </button>
            {open && (
                <div
                    role="menu"
                    className="yz-pop absolute left-0 top-[32px] z-30 w-[150px] rounded-[11px] border border-(--line-2) bg-(--frost-light) p-[5px] shadow-[var(--shadow-xl)] backdrop-blur-[20px]"
                >
                    {options.map((opt) => {
                        const active = opt.label === value
                        return (
                            <button
                                key={opt.key}
                                type="button"
                                role="menuitem"
                                onClick={() => {
                                    onSelect(opt.key)
                                    setOpen(false)
                                }}
                                className={
                                    "flex h-[30px] w-full items-center gap-[8px] rounded-[8px] px-[11px] text-left text-[12px] text-(--ink-1) transition-colors hover:bg-(--yz-hover) " +
                                    (active ? "bg-(--yz-active) font-semibold" : "font-medium")
                                }
                            >
                                {opt.label}
                            </button>
                        )
                    })}
                </div>
            )}
        </div>
    )
}

/**
 * §2 L743-868 Log tab. Owns the data lifecycle for the commit log (first-page
 * load on mount, reload when HEAD moves, reset on repo switch), the filter
 * toolbar (search + User + Date), and lays out the graph/list (LogGraph) and the
 * details panel (CommitDetails).
 *
 * Exposed for the Diff modal: `onOpenFile(hash, file)` fires when a changed-file
 * row is clicked (open that file at the commit); `onCompare(hash)` fires from the
 * Compare footer button (open the whole commit). Both are injected by GitPanel.
 */
export function LogTab({
    onOpenFile,
    onCompare
}: {
    onOpenFile?: (hash: string, file: CommitFileChange) => void
    onCompare?: (hash: string) => void
}) {
    const commits = useGitLogStore((s) => s.commits)
    const hasMore = useGitLogStore((s) => s.hasMore)
    const loading = useGitLogStore((s) => s.loading)
    const loadingMore = useGitLogStore((s) => s.loadingMore)
    const error = useGitLogStore((s) => s.error)
    const filters = useGitLogStore((s) => s.filters)
    const authors = useGitLogStore((s) => s.authors)
    const selectedHash = useGitLogStore((s) => s.selectedHash)
    const detail = useGitLogStore((s) => s.detail)
    const detailLoading = useGitLogStore((s) => s.detailLoading)

    const loadFirstPage = useGitLogStore((s) => s.loadFirstPage)
    const loadMore = useGitLogStore((s) => s.loadMore)
    const setFilters = useGitLogStore((s) => s.setFilters)
    const select = useGitLogStore((s) => s.select)

    const runOp = useGitStore((s) => s.runOp)
    const status = useGitStore((s) => s.status)
    const busy = useGitStore((s) => s.busy)

    // HEAD-change signal. branch + headOid change together on commit / checkout /
    // reset / merge but NOT on plain working-tree edits (those touch the file
    // buckets, not headOid) — so the log reloads exactly when history moves,
    // avoiding a reload on every fs event. ahead/behind deliberately excluded:
    // they shift on fetch without HEAD moving.
    const branch = status?.branch ?? null
    const headOid = status?.headOid ?? null

    // repo switch → reset the log store, then reload under the new root.
    const workspacePath = useWorkspaceStore((s) => s.workspacePath)

    // Local search-box text so debounce doesn't fight the controlled input.
    const [query, setQuery] = useState(filters.query)
    const queryTimer = useRef<ReturnType<typeof setTimeout> | null>(null)
    const [checkoutNotice, setCheckoutNotice] = useState<string | null>(null)

    // Reset + reload when the repo root changes (covers first mount too). A
    // repo switch also changes branch/headOid, so re-arm the head-skip below to
    // avoid a redundant second load from the HEAD effect firing on the same
    // change.
    const firstHead = useRef(true)
    useEffect(() => {
        useGitLogStore.getState().reset()
        setQuery("")
        firstHead.current = true
        void loadFirstPage()
        // eslint-disable-next-line react-hooks/exhaustive-deps
    }, [workspacePath])

    // Reload when HEAD moves (new commit / checkout / reset). Skipped on the very
    // first render pair via a ref so it doesn't double-load alongside mount.
    useEffect(() => {
        if (firstHead.current) {
            firstHead.current = false
            return
        }
        void loadFirstPage()
        // eslint-disable-next-line react-hooks/exhaustive-deps
    }, [branch, headOid])

    // Clear a pending debounce on unmount so a trailing timer can't fire after
    // the input is gone. Without this, "type → switch tab → switch back" inside
    // the 250ms window would remount with a reset input, then the old timer fires
    // setFilters({query: old}) — desyncing filters from the empty input.
    useEffect(() => {
        return () => {
            if (queryTimer.current) clearTimeout(queryTimer.current)
        }
    }, [])

    function onQueryChange(next: string) {
        setQuery(next)
        if (queryTimer.current) clearTimeout(queryTimer.current)
        queryTimer.current = setTimeout(() => {
            setFilters({ query: next })
        }, QUERY_DEBOUNCE_MS)
    }

    async function onCheckout(hash: string) {
        // Dirty-tab protection mirrors BranchPopover: block checkout while any
        // editor tab has unsaved changes. checkout runs through gitStore.runOp so
        // it shares the Console log + busy gate; on success the git:state-changed
        // event + our HEAD-change effect refresh the log.
        const dirty = useWorkspaceStore
            .getState()
            .groups.some((g) => g.tabs.some((t) => t.dirty))
        if (dirty) {
            setCheckoutNotice("有未儲存的變更，請先存檔或放棄")
            return
        }
        setCheckoutNotice(null)
        const ok = await runOp("checkout", () => gitCheckout(hash))
        if (ok) void logUserAction("git_checkout", `checkout ${hash.slice(0, 7)}`)
    }

    const selectedCommit = commits.find((c) => c.hash === selectedHash) ?? null
    const cherryPickDisabled =
        !!status?.inProgress || (status?.conflicted?.length ?? 0) > 0 || !!busy

    // User dropdown options: "All" + author names (§brief filters.author = name).
    const userOptions = [
        { key: "__all__", label: "All" },
        ...authors.map((a) => ({ key: a.name, label: a.name }))
    ]
    const userValue = filters.author ?? "All"
    const dateValue =
        DATE_OPTIONS.find((o) => o.since === filters.since)?.label ?? "All"

    return (
        <div className="flex min-h-0 flex-1 flex-col">
            {/* §2 L744-759 filter toolbar */}
            <div className="flex h-[40px] shrink-0 items-center gap-[8px] border-b border-(--line-1) bg-(--yz-sunk) px-[12px]">
                <div className="flex h-[28px] shrink grow-0 basis-[300px] items-center gap-[8px] rounded-[9px] border border-(--line-1) bg-(--yz-solid) px-[11px] shadow-(--shadow-xs)">
                    <svg
                        width="13"
                        height="13"
                        viewBox="0 0 24 24"
                        fill="none"
                        stroke="var(--ink-3)"
                        strokeWidth="2"
                        strokeLinecap="round"
                        strokeLinejoin="round"
                        className="shrink-0"
                        aria-hidden="true"
                    >
                        <circle cx="11" cy="11" r="7" />
                        <path d="m21 21-4.3-4.3" />
                    </svg>
                    <input
                        value={query}
                        onChange={(e) => onQueryChange(e.target.value)}
                        placeholder="Filter by message, author or hash…"
                        aria-label="Filter commits"
                        className="min-w-0 flex-1 border-none bg-transparent font-sans text-[12px] text-(--ink-1) outline-none"
                    />
                </div>

                <FilterDropdown
                    field="User"
                    value={userValue}
                    options={userOptions}
                    onSelect={(key) =>
                        setFilters({ author: key === "__all__" ? null : key })
                    }
                />
                <FilterDropdown
                    field="Date"
                    value={dateValue}
                    options={DATE_OPTIONS.map((o) => ({ key: o.label, label: o.label }))}
                    onSelect={(key) => {
                        const opt = DATE_OPTIONS.find((o) => o.label === key)
                        setFilters({ since: opt?.since ?? null })
                    }}
                />
            </div>

            {checkoutNotice && (
                <div
                    className="border-b border-(--line-1) px-[12px] py-[6px] text-[11px]"
                    style={{ background: "var(--danger-soft)", color: "var(--status-d)" }}
                >
                    {checkoutNotice}
                </div>
            )}

            {/* body: graph + list | details */}
            <div className="flex min-h-0 flex-1">
                <div className="flex min-w-0 flex-1 flex-col">
                    {loading ? (
                        <div className="flex flex-1 items-center justify-center text-[12.5px] text-(--ink-3)">
                            Loading commits…
                        </div>
                    ) : error ? (
                        <div className="flex flex-1 flex-col items-center justify-center gap-[10px] px-[16px] text-center">
                            <span className="text-[12.5px] text-(--ink-2)">{error}</span>
                            <button
                                type="button"
                                onClick={() => void loadFirstPage()}
                                className="h-[28px] rounded-[9px] border border-(--line-1) bg-(--yz-solid) px-[12px] text-[11.5px] font-semibold text-(--ink-1) shadow-(--shadow-xs) transition-colors hover:bg-(--paper-1)"
                            >
                                Retry
                            </button>
                        </div>
                    ) : commits.length === 0 ? (
                        <div className="flex flex-1 items-center justify-center text-[12.5px] text-(--ink-3)">
                            No commits match the current filters.
                        </div>
                    ) : (
                        <LogGraph
                            commits={commits}
                            selectedHash={selectedHash}
                            onSelect={(hash) => void select(hash)}
                            hasMore={hasMore}
                            loadingMore={loadingMore}
                            onLoadMore={() => void loadMore()}
                        />
                    )}
                </div>

                <CommitDetails
                    selectedCommit={selectedCommit}
                    detail={detail}
                    detailLoading={detailLoading}
                    onCheckout={(hash) => void onCheckout(hash)}
                    onOpenFile={
                        onOpenFile && selectedCommit
                            ? (file) => onOpenFile(selectedCommit.hash, file)
                            : undefined
                    }
                    onCompare={onCompare}
                    onCherryPick={(hash) => void runOp("cherry-pick", () => gitCherryPick(hash))}
                    cherryPickDisabled={cherryPickDisabled}
                />
            </div>
        </div>
    )
}

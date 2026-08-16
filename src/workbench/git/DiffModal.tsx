import { Button } from "@/components/ui/button"
import { Input } from "@/components/ui/input"
import { ResizableHandle, ResizablePanel, ResizablePanelGroup } from "@/components/ui/resizable"
import { ScrollArea } from "@/components/ui/scroll-area"
import { ToggleGroup, ToggleGroupItem } from "@/components/ui/toggle-group"
import { DialogResizeHandles } from "@/components/ui/dialog-resize-handles"
import { useResizableDialogSize } from "@/hooks/useResizableDialogSize"
import { dialogMinSize } from "@/lib/dialogSize"
import { useEffect, useLayoutEffect, useMemo, useRef, useState } from "react"
import type { PanelImperativeHandle } from "react-resizable-panels"
import { Dialog as DialogPrimitive } from "radix-ui"
import { useTranslation } from "react-i18next"
import {
    FILE_FILTER_MIN_COUNT,
    filterRowsByPath,
    moveListIndex
} from "@/workbench/git/diffPreview"

import i18n from "@/lib/i18n"
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
import { DiffFilesToggle } from "@/workbench/git/DiffFilesToggle"

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
// commit → shortHash + subject; text → caller-provided title. Called from
// DiffModal's render body (not a hook itself), so it reads the current
// language straight off the shared i18n singleton — DiffModal's own
// useTranslation("menus") call already re-renders it on language change.
function sourceHeader(source: DiffModalSource): { title: string; sub: string } {
    if (source.type === "worktree") {
        const n = source.files.length
        return {
            title: i18n.t("diffModal.workingTree", { ns: "menus" }),
            sub: i18n.t("diffModal.changedFileCount", { ns: "menus", count: n })
        }
    }
    if (source.type === "text") {
        return { title: source.title, sub: i18n.t("diffModal.agentDiffSub", { ns: "menus" }) }
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
    side: "staged" | "unstaged" | null
}

function sourceRows(source: DiffModalSource): Row[] {
    if (source.type === "worktree") {
        return source.files.map((f) => ({
            path: f.path,
            badge: badgeChar(f.status),
            cacheKey: `${f.staged ? "s" : "c"}:${f.path}`,
            side: f.staged ? "staged" : "unstaged"
        }))
    }
    if (source.type === "text") {
        return [{ path: source.title, badge: "M", cacheKey: `text:${source.title}`, side: null }]
    }
    // Commit files have a single side per path — key by path (unchanged).
    return source.files.map((f) => ({ path: f.path, badge: badgeChar(f.status), cacheKey: f.path, side: null }))
}

// Load the diff for the file at `index`, keyed by path in a per-open cache. The
// commit/worktree branch is chosen from the source; a stale response (activeIndex
// moved on before it resolved) is dropped by the caller via the token check.
function loadDiffFor(source: DiffModalSource, index: number): Promise<DiffContent> {
    if (source.type === "worktree") {
        const f = source.files[index]
        return loadWorktreeDiff(source.repositoryRoot, f.path, f.staged, f.origPath)
    }
    if (source.type === "text") {
        return Promise.resolve({ original: source.original, modified: source.modified })
    }
    const f = source.files[index]
    return loadCommitDiff(source.repositoryRoot, source.hash, source.parents, f)
}

type IndexedRow = Row & { index: number }

function DiffFileOption({
    row,
    index,
    visibleIndex,
    selected,
    tabbable,
    sideLabel,
    onSelect,
    onKeyDown
}: {
    row: Row
    index: number
    visibleIndex: number
    selected: boolean
    tabbable: boolean
    sideLabel?: string
    onSelect: (index: number) => void
    onKeyDown: (event: React.KeyboardEvent, visibleIndex: number) => void
}) {
    const { name, dir } = splitPath(row.path)
    return (
        <Button
            id={`diff-file-${index}`}
            type="button"
            variant="ghost"
            role="option"
            aria-selected={selected}
            tabIndex={tabbable ? 0 : -1}
            title={row.path}
            aria-label={sideLabel ? `${row.path} (${sideLabel})` : row.path}
            onClick={() => onSelect(index)}
            onKeyDown={(event) => onKeyDown(event, visibleIndex)}
            className={
                "my-[1px] flex h-[32px] w-full items-center gap-[9px] rounded-[8px] px-[8px] text-left transition-[background] duration-[120ms] " +
                (selected ? "bg-(--yz-active) shadow-(--shadow-xs)" : "hover:bg-(--yz-panel)")
            }
        >
            <FileBadge badge={row.badge} />
            <span className="min-w-0 flex-1 truncate">
                <span className={"text-[12px] " + (selected ? "font-semibold text-(--ink-0)" : "font-medium text-(--ink-1)")}>
                    {name}
                </span>
                {sideLabel && <span className="ml-[6px] text-[10px] font-semibold text-(--ink-3)">{sideLabel}</span>}
                {dir && <span className="ml-[6px] text-[10px] text-(--ink-4)">{dir}</span>}
            </span>
        </Button>
    )
}

function DiffFileGroup({
    label,
    rows,
    activeIndex,
    focusIndex,
    sideLabel,
    onSelect,
    onKeyDown,
    visibleRows
}: {
    label: string
    rows: IndexedRow[]
    activeIndex: number
    focusIndex: number | null
    sideLabel: string
    onSelect: (index: number) => void
    onKeyDown: (event: React.KeyboardEvent, visibleIndex: number) => void
    visibleRows: IndexedRow[]
}) {
    if (rows.length === 0) return null
    return (
        <div className="mb-[6px]">
            <div className="flex items-center gap-[6px] px-[8px] pt-[5px] pb-[4px] text-[9.5px] font-semibold tracking-[0.08em] text-(--ink-3) uppercase">
                <span>{label}</span>
                <span className="font-mono text-(--ink-4)">{rows.length}</span>
            </div>
            {rows.map((row) => {
                const visibleIndex = visibleRows.findIndex((candidate) => candidate.index === row.index)
                return (
                    <DiffFileOption
                        key={row.cacheKey}
                        row={row}
                        index={row.index}
                        visibleIndex={visibleIndex}
                        selected={row.index === activeIndex}
                        tabbable={focusIndex === row.index}
                        sideLabel={sideLabel}
                        onSelect={onSelect}
                        onKeyDown={onKeyDown}
                    />
                )
            })}
        </div>
    )
}

/**
 * §D (design L1393-1465) Diff viewer modal — app-level, mounted in AppShell.
 * Header (title/sub + Unified/Split toggle + close), a left file list, and the
 * right diff pane which reuses DiffView. Worktree and commit sources load their
 * text through diffLoad. Built on the shadcn Dialog for Esc / focus-trap / a11y,
 * with the design's own overlay + panel styling.
 */
export function DiffModal() {
    const { t } = useTranslation("menus")
    const { t: tc } = useTranslation("common")
    const open = useDiffModalStore((s) => s.open)
    const liveSource = useDiffModalStore((s) => s.source)
    const [heldSource, setHeldSource] = useState(liveSource)
    if (liveSource && liveSource !== heldSource) setHeldSource(liveSource)
    const source = liveSource ?? heldSource
    const previousFocusRef = useRef<HTMLElement | null>(null)
    const closeButtonRef = useRef<HTMLButtonElement>(null)
    const activeIndex = useDiffModalStore((s) => s.activeIndex)
    const sourceGeneration = useDiffModalStore((s) => s.sourceGeneration)
    const mode = useDiffModalStore((s) => s.mode)
    const setActive = useDiffModalStore((s) => s.setActive)
    const setMode = useDiffModalStore((s) => s.setMode)
    const close = useDiffModalStore((s) => s.close)
    const sizing = useResizableDialogSize({
        resizeId: "git-diff",
        minSize: dialogMinSize(640, 400),
    })

    const [loadState, setLoadState] = useState<{
        identity: string
        diff: DiffContent | null
        error: string | null
    } | null>(null)
    const [retryToken, setRetryToken] = useState(0)
    const [fileFilter, setFileFilter] = useState("")
    // Collapse chrome is bound to sourceGeneration so a leftover collapsed=true
    // from the previous heldSource session derives to expanded on the next open*.
    // Mount-time onCollapse (0px first resize) must not stamp this generation.
    const [filesPanel, setFilesPanel] = useState({ sourceGeneration: -1, collapsed: false })
    const filesCollapsed = filesPanel.sourceGeneration === sourceGeneration && filesPanel.collapsed
    const filesPanelRef = useRef<PanelImperativeHandle>(null)
    const filePanelContentRef = useRef<HTMLDivElement>(null)
    const expandFilesRef = useRef<HTMLButtonElement>(null)
    // Per-open cache path→loaded diff. Capture a dedicated Map for each source
    // generation so a late response from source A can never write into source B.
    const cacheRef = useRef<{ sourceGeneration: number; values: Map<string, DiffContent> }>({
        sourceGeneration: -1,
        values: new Map()
    })
    const requestGenerationRef = useRef<Map<string, number>>(new Map())

    const rows = source ? sourceRows(source) : []
    const activeRow = rows[activeIndex] ?? null
    const activeIdentity = source && activeRow
        ? `${sourceGeneration}:${source.type === "text" ? "text" : source.repositoryRoot}:${activeRow.cacheKey}`
        : ""
    const diff = loadState?.identity === activeIdentity ? loadState.diff : null
    const loadError = loadState?.identity === activeIdentity ? loadState.error : null

    // Load (or serve from cache) the active file's diff. Stale responses are
    // dropped when the active row or source generation changed before resolve.
    useEffect(() => {
        if (!source || !activeRow) {
            setLoadState(null)
            return
        }
        const identity = activeIdentity
        const key = activeRow.cacheKey
        if (cacheRef.current.sourceGeneration !== sourceGeneration) {
            cacheRef.current = { sourceGeneration, values: new Map() }
            requestGenerationRef.current = new Map()
        }
        const cacheContext = cacheRef.current
        const cached = cacheContext.values.get(key)
        if (cached) {
            setLoadState({ identity, diff: cached, error: null })
            return
        }
        setLoadState(null)
        let cancelled = false
        const generation = (requestGenerationRef.current.get(key) ?? 0) + 1
        requestGenerationRef.current.set(key, generation)
        void loadDiffFor(source, activeIndex)
            .then((content) => {
                // A request is allowed to populate the cache only while it is
                // still the newest request for this exact source+row identity.
                if (
                    cancelled
                    || cacheRef.current !== cacheContext
                    || requestGenerationRef.current.get(key) !== generation
                ) return
                cacheContext.values.set(key, content)
                setLoadState({ identity, diff: content, error: null })
            })
            .catch((error: unknown) => {
                if (
                    cancelled
                    || cacheRef.current !== cacheContext
                    || requestGenerationRef.current.get(key) !== generation
                ) return
                setLoadState({
                    identity,
                    diff: null,
                    error: error instanceof Error ? error.message : String(error)
                })
            })
        return () => {
            cancelled = true
        }
        // Depend on cacheKey/identity, not the activeRow object: sourceRows()
        // returns a fresh object each render, and putting that in deps would
        // re-fire the cache-hit setLoadState path forever.
        // eslint-disable-next-line react-hooks/exhaustive-deps
    }, [source, sourceGeneration, activeIndex, activeRow?.cacheKey, activeIdentity, retryToken])

    useLayoutEffect(() => {
        if (!open) return
        filesPanelRef.current?.expand()
    }, [open, sourceGeneration])

    useLayoutEffect(() => {
        if (!filesCollapsed) return
        const panel = filePanelContentRef.current
        const expand = expandFilesRef.current
        if (!panel || !expand) return
        const active = document.activeElement
        if (active instanceof Node && panel.contains(active)) expand.focus()
    }, [filesCollapsed])

    const stats = useMemo(() => (diff ? diffStats(diff) : null), [diff])

    if (!source) return null

    const { title, sub } = sourceHeader(source)
    const indexedRows = rows.map((row, index) => ({ row, index }))
    const visibleRows = filterRowsByPath(indexedRows.map(({ row, index }) => ({ ...row, index })), fileFilter)
    const focusRow = visibleRows.find((row) => row.index === activeIndex) ?? visibleRows[0] ?? null
    const focusIndex = focusRow?.index ?? null
    const showFilter = rows.length > FILE_FILTER_MIN_COUNT
    const worktreeGroups = source.type === "worktree"
        ? {
            staged: visibleRows.filter((row) => row.side === "staged"),
            unstaged: visibleRows.filter((row) => row.side === "unstaged")
        }
        : null

    function toggleFilesPanel() {
        const panel = filesPanelRef.current
        if (!panel) return
        if (panel.isCollapsed()) panel.expand()
        else panel.collapse()
    }

    function onFilesPanelCollapse() {
        setFilesPanel({ sourceGeneration, collapsed: true })
    }

    function onFilesPanelExpand() {
        setFilesPanel({ sourceGeneration, collapsed: false })
    }

    function focusOption(index: number) {
        document.getElementById(`diff-file-${index}`)?.focus()
    }

    function onFileListKeyDown(event: React.KeyboardEvent, currentVisible: number) {
        const nextVisible = moveListIndex(currentVisible, visibleRows.length, event.key)
        if (nextVisible != null) {
            event.preventDefault()
            const next = visibleRows[nextVisible]
            if (!next) return
            setActive(next.index)
            focusOption(next.index)
            return
        }
        if (event.key === "Enter" || event.key === " ") {
            event.preventDefault()
            const current = visibleRows[currentVisible]
            if (current) setActive(current.index)
        }
    }
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
                {/* Panel: shared 80% viewport sizing + per-modal resize; keep absolute/no-Portal. */}
                <DialogPrimitive.Content
                    aria-label={t("diffModal.title", { title })}
                    data-dialog-size-id="git-diff"
                    data-resizing={sizing.isResizing ? "true" : undefined}
                    onOpenAutoFocus={(event) => {
                        event.preventDefault()
                        const related = (event as { relatedTarget?: EventTarget | null }).relatedTarget
                        if (related instanceof HTMLElement) previousFocusRef.current = related
                        else if (document.activeElement instanceof HTMLElement) previousFocusRef.current = document.activeElement
                        ;(closeButtonRef.current ?? expandFilesRef.current)?.focus()
                    }}
                    onCloseAutoFocus={(event) => {
                        event.preventDefault()
                        const restore = previousFocusRef.current
                        if (restore?.isConnected) restore.focus()
                    }}
                    data-diff-surface=""
                    className={`yz-diffin absolute top-1/2 left-1/2 z-[62] flex min-h-0 max-w-none -translate-x-1/2 -translate-y-1/2 flex-col overflow-hidden rounded-(--r-lg) border border-(--line-2) bg-(--paper-0) shadow-(--shadow-xl) outline-none${sizing.isResizing ? " duration-0" : ""}`}
                    style={sizing.style}
                >
                    <DialogPrimitive.Title className="sr-only">
                        {t("diffModal.title", { title })}
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
                        <div data-testid="diff-modal-title" className="flex min-w-0 flex-1 flex-col gap-[1px] overflow-hidden">
                            <div className="min-w-0 truncate overflow-hidden font-serif text-[15px] font-semibold leading-[1.1] text-(--ink-0)">
                                {t("diffModal.title", { title })}
                            </div>
                            <div className="min-w-0 truncate font-mono text-[10.5px] text-(--ink-3)">{sub}</div>
                        </div>
                        {/* §4.2 unified/split toggle */}
                        <ToggleGroup
                            type="single"
                            value={mode}
                            onValueChange={(value) => value && setMode(value as DiffMode)}
                            className="flex shrink-0 gap-[3px] rounded-[9px] bg-(--yz-sunk) p-[3px]"
                        >
                            {(["unified", "split"] as const).map((m: DiffMode) => (
                                <ToggleGroupItem
                                    key={m}
                                    value={m}
                                    aria-label={m === "unified" ? t("diffModal.unified") : t("diffModal.split")}
                                    className="h-[26px] rounded-[7px] px-[12px] text-[11px] font-semibold transition-all duration-[140ms] data-[state=on]:bg-(--yz-solid) data-[state=on]:text-(--ink-0) data-[state=on]:shadow-(--shadow-xs) data-[state=off]:text-(--ink-3)"
                                >
                                    {m === "unified" ? t("diffModal.unified") : t("diffModal.split")}
                                </ToggleGroupItem>
                            ))}
                        </ToggleGroup>
                        <Button
                            ref={closeButtonRef}
                            variant="ghost"
                            size="icon-sm"
                            type="button"
                            aria-label={t("diffModal.close")}
                            title={t("diffModal.close")}
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
                        </Button>
                    </div>

                    <ResizablePanelGroup
                        id="diff-modal-layout"
                        orientation="horizontal"
                        className="min-h-0 flex-1"
                    >
                        <ResizablePanel
                            id="diff-files"
                            panelRef={filesPanelRef}
                            defaultSize="24"
                            minSize="15"
                            maxSize="40"
                            collapsible
                            collapsedSize="0"
                            onCollapse={onFilesPanelCollapse}
                            onExpand={onFilesPanelExpand}
                            className="min-h-0"
                            data-files-collapsed={filesCollapsed ? "true" : "false"}
                        >
                            <div
                                ref={filePanelContentRef}
                                id="diff-file-panel-content"
                                data-testid="diff-file-panel-content"
                                inert={filesCollapsed || undefined}
                                aria-hidden={filesCollapsed || undefined}
                                className="flex h-full min-h-0 flex-col border-r border-(--line-1) bg-(--paper-1)"
                            >
                                <div className="flex h-[36px] shrink-0 items-center gap-[6px] px-[10px]">
                                    <span className="min-w-0 flex-1 truncate text-[9.5px] font-semibold tracking-[0.08em] text-(--ink-3) uppercase">
                                        {t("diffModal.filesLabel")}
                                    </span>
                                </div>
                                {showFilter && (
                                    <div className="shrink-0 px-[9px] pb-[6px]">
                                        <Input
                                            value={fileFilter}
                                            onChange={(event) => setFileFilter(event.target.value)}
                                            aria-label={t("diffModal.filterFiles")}
                                            placeholder={t("diffModal.filterFilesPlaceholder")}
                                            className="h-[28px] text-[12px]"
                                        />
                                    </div>
                                )}
                                <ScrollArea className="min-h-0 flex-1" viewportClassName="p-[9px]">
                                    <div
                                        role="listbox"
                                        aria-label={t("diffModal.filesLabel")}
                                        aria-activedescendant={focusRow ? `diff-file-${focusRow.index}` : undefined}
                                        data-testid="diff-file-list"
                                    >
                                        {worktreeGroups ? (
                                            <>
                                                <DiffFileGroup
                                                    label={t("diffModal.stagedGroup")}
                                                    rows={worktreeGroups.staged}
                                                    activeIndex={activeIndex}
                                                    focusIndex={focusIndex}
                                                    sideLabel={t("diffModal.stagedSide")}
                                                    onSelect={setActive}
                                                    onKeyDown={onFileListKeyDown}
                                                    visibleRows={visibleRows}
                                                />
                                                <DiffFileGroup
                                                    label={t("diffModal.unstagedGroup")}
                                                    rows={worktreeGroups.unstaged}
                                                    activeIndex={activeIndex}
                                                    focusIndex={focusIndex}
                                                    sideLabel={t("diffModal.unstagedSide")}
                                                    onSelect={setActive}
                                                    onKeyDown={onFileListKeyDown}
                                                    visibleRows={visibleRows}
                                                />
                                            </>
                                        ) : (
                                            visibleRows.map((row, visibleIndex) => (
                                                <DiffFileOption
                                                    key={row.cacheKey}
                                                    row={row}
                                                    index={row.index}
                                                    visibleIndex={visibleIndex}
                                                    selected={row.index === activeIndex}
                                                    tabbable={focusIndex === row.index}
                                                    onSelect={setActive}
                                                    onKeyDown={onFileListKeyDown}
                                                />
                                            ))
                                        )}
                                    </div>
                                </ScrollArea>
                            </div>
                        </ResizablePanel>
                        <ResizableHandle
                            id="diff-files-handle"
                            data-testid="diff-files-handle"
                            disabled={filesCollapsed}
                            aria-hidden={filesCollapsed || undefined}
                            className={filesCollapsed ? "pointer-events-none w-0 overflow-hidden border-0 bg-transparent after:hidden" : undefined}
                        />
                        <ResizablePanel id="diff-body" defaultSize="76" minSize="60" className="min-h-0">
                            <div className="relative flex h-full min-w-0 flex-col">
                                <div className="flex h-[38px] shrink-0 items-center gap-[9px] border-b border-(--line-1) bg-(--yz-sunk) px-[10px]">
                                    <DiffFilesToggle
                                        ref={expandFilesRef}
                                        collapsed={filesCollapsed}
                                        label={filesCollapsed ? t("diffModal.expandFiles") : t("diffModal.collapseFiles")}
                                        controlsId="diff-file-panel-content"
                                        onToggle={toggleFilesPanel}
                                    />
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
                                        <DiffView content={diff} mode={mode} path={activePath} />
                                    ) : loadError ? (
                                        <div className="flex h-full flex-col items-center justify-center gap-[10px] px-[16px] text-center">
                                            <p role="alert" className="text-[12.5px] text-(--ink-2)">
                                                {t("diffModal.loadFailed", { message: loadError })}
                                            </p>
                                            <Button
                                                type="button"
                                                size="xs"
                                                onClick={() => setRetryToken((token) => token + 1)}
                                            >
                                                {t("diffModal.retry")}
                                            </Button>
                                        </div>
                                    ) : (
                                        <div className="flex h-full items-center justify-center text-[12.5px] text-(--ink-3)">
                                            {t("diffModal.loadingEllipsis")}
                                        </div>
                                    )}
                                </div>
                            </div>
                        </ResizablePanel>
                    </ResizablePanelGroup>
                    <DialogResizeHandles
                        sizing={sizing}
                        resizeLabel={tc("dialog.resize")}
                        resetSizeLabel={tc("dialog.resetSize")}
                    />
                </DialogPrimitive.Content>
            </>
        </DialogPrimitive.Root>
    )
}

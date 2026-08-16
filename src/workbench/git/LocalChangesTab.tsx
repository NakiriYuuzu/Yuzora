import { useEffect, useLayoutEffect, useMemo, useRef, useState, type CSSProperties, type KeyboardEvent as ReactKeyboardEvent, type MouseEvent as ReactMouseEvent } from "react"
import type { PanelImperativeHandle } from "react-resizable-panels"
import { useTranslation } from "react-i18next"

import { gitStage, gitUnstage } from "../../lib/ipc"
import { logUserAction } from "@/features/logs/userAction"
import type { DiffContent } from "../../lib/types"
import { useGitStore } from "../../state/gitStore"
import { useUiStore } from "../../state/uiStore"
import { diffStats, langLabel, loadWorktreeDiff, splitPath } from "./diffLoad"
import { DiffView } from "./DiffView"
import { GitBadge } from "./fileRows"
import { openGitChangeContextMenu } from "./gitChangeContextMenu"
import {
    buildGitChangeModel,
    gitChangeDomId,
    gitChangeId,
    gitChangeIdSet,
    isGitToggleModifier,
    sameGitChange,
    uniquePaths,
    type GitChangeRow,
    type GitSectionKey
} from "./gitChangeSelection"
import {
    VirtualizedGitChangeList,
    type GitChangeVirtualItem,
    type VirtualizedGitChangeListHandle,
    gitChangeVirtualItems
} from "./VirtualizedGitChangeList"
import { DiffFilesToggle } from "./DiffFilesToggle"
import { Button } from "@/components/ui/button"
import { Input } from "@/components/ui/input"
import { ResizableHandle, ResizablePanel, ResizablePanelGroup } from "@/components/ui/resizable"
import { ToggleGroup, ToggleGroupItem } from "@/components/ui/toggle-group"
import {
    FILE_FILTER_MIN_COUNT,
    filterRowsByPath,
    moveListIndex
} from "@/workbench/git/diffPreview"

type DiffMode = "unified" | "split"

// Local-changes file row: badge + name/dir + stage/unstage control.
// Keyboard: Enter/Space selects the row; the stage button is a real button.
function FileRow({
    row,
    selected,
    tabbable,
    optionId,
    onSelect,
    onContextMenu,
    onStageToggle,
    onListKeyDown,
    disabled,
    style
}: {
    row: GitChangeRow
    selected: boolean
    tabbable: boolean
    optionId: string
    onSelect: (event: ReactMouseEvent) => void
    onContextMenu: (event: ReactMouseEvent) => void
    onStageToggle: () => void
    onListKeyDown: (event: ReactKeyboardEvent) => void
    disabled: boolean
    style?: CSSProperties
}) {
    const { t } = useTranslation("menus")
    const { name, dir } = splitPath(row.path)
    return (
        <li style={style} className={
            "group flex h-[32px] items-center gap-[2px] rounded-[8px] px-[2px] " +
            (selected ? "bg-(--yz-active) shadow-(--shadow-xs)" : "hover:bg-(--yz-panel)")
        }>
            <Button
                id={optionId}
                type="button"
                variant="ghost"
                role="option"
                aria-selected={selected}
                aria-pressed={selected}
                tabIndex={tabbable ? 0 : -1}
                title={row.path}
                onClick={onSelect}
                onContextMenu={onContextMenu}
                onKeyDown={onListKeyDown}
                className="flex h-[28px] min-w-0 flex-1 items-center justify-start gap-[9px] rounded-[6px] px-[6px] text-left focus-visible:ring-2 focus-visible:ring-(--yz-accent)"
            >
                <GitBadge badge={row.badge} />
                <span className="flex min-w-0 flex-1 items-baseline">
                <span
                    className={
                        "truncate text-[12.5px] " +
                        (selected ? "font-semibold text-(--ink-0)" : "font-medium text-(--ink-1)")
                    }
                >
                    {name}
                </span>
                {dir && (
                    <span className="ml-[6px] min-w-0 truncate text-[10px] font-normal text-(--ink-4)">{dir}</span>
                )}
                </span>
            </Button>
            <Button
                type="button"
                variant="ghost"
                size="icon-xs"
                disabled={disabled}
                aria-label={
                    row.staged
                        ? t("localChangesTab.unstageFileAriaLabel", { path: row.path })
                        : t("localChangesTab.stageFileAriaLabel", { path: row.path })
                }
                title={
                    row.staged
                        ? t("localChangesTab.unstageFileTitle")
                        : t("localChangesTab.stageFileTitle")
                }
                onClick={onStageToggle}
                onKeyDown={(event) => {
                    // Explicit activation so keyboard tests exercise a real handler
                    // without relying on jsdom's incomplete button Space/Enter click.
                    if (event.key === "Enter" || event.key === " ") {
                        event.preventDefault()
                        event.stopPropagation()
                        if (!disabled) onStageToggle()
                    }
                }}
                className="flex size-[22px] shrink-0 items-center justify-center rounded-[6px] text-(--ink-3) transition-all duration-[130ms] hover:bg-(--yz-hover) hover:text-(--yz-accent-ink) focus-visible:bg-(--yz-hover) focus-visible:text-(--yz-accent-ink) focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-(--yz-accent)"
            >
                <svg
                    width="14"
                    height="14"
                    viewBox="0 0 24 24"
                    fill="none"
                    stroke="currentColor"
                    strokeWidth="2"
                    strokeLinecap="round"
                    aria-hidden="true"
                >
                    {row.staged ? <path d="M5 12h14" /> : <path d="M12 5v14M5 12h14" />}
                </svg>
            </Button>
        </li>
    )
}

/**
 * GitPanel → Local changes tab.
 * Left: responsive file list (min ~224px). Right: selected-file diff.
 * Product default is split; CodeMirror owns scrolling.
 */
export function LocalChangesTab() {
    const { t } = useTranslation("menus")
    const status = useGitStore((s) => s.status)
    const runOp = useGitStore((s) => s.runOp)
    const busy = useGitStore((s) => s.busy)
    const snapshotStale = useGitStore((s) => s.snapshotStale)
    const statusRevision = useGitStore((s) => s.statusRevision)
    const repositoryRoot = useGitStore((s) =>
        s.environment?.status === "ready" ? s.environment.root : null
    )
    const selectedPath = useUiStore((s) => s.gitSelectedPath)
    const selectedStaged = useUiStore((s) => s.gitSelectedStaged)
    const selection = useUiStore((s) => s.gitChangeSelection)
    const primary = useUiStore((s) => s.gitChangePrimary)
    const selectGitChange = useUiStore((s) => s.selectGitChange)
    const reconcileGitChangeSelection = useUiStore((s) => s.reconcileGitChangeSelection)

    const diffMode = useUiStore((s) => s.gitDiffMode)
    const setDiffMode = useUiStore((s) => s.setGitDiffMode)
    const [loadState, setLoadState] = useState<{
        identity: string
        diff: DiffContent | null
        error: string | null
    } | null>(null)
    const [retryToken, setRetryToken] = useState(0)
    const [fileFilter, setFileFilter] = useState("")
    const [filesCollapsed, setFilesCollapsed] = useState(false)
    const filesPanelRef = useRef<PanelImperativeHandle>(null)
    const filePanelContentRef = useRef<HTMLDivElement>(null)
    const expandFilesRef = useRef<HTMLButtonElement>(null)
    const virtualListRef = useRef<VirtualizedGitChangeListHandle>(null)

    const model = useMemo(() => buildGitChangeModel(status), [status])
    const { rows, visualOrder, buckets } = model
    const visibleOrder = useMemo(
        () => filterRowsByPath(visualOrder, fileFilter),
        [visualOrder, fileFilter]
    )
    const visibleIds = useMemo(() => gitChangeIdSet(visibleOrder), [visibleOrder])
    const virtualItems = useMemo(() => gitChangeVirtualItems(model, {
        rowMatches: (row) => visibleIds.has(gitChangeId(row))
    }), [model, visibleIds])
    const virtualIndexById = useMemo(() => {
        const indexes = new Map<string, number>()
        virtualItems.forEach((item, index) => {
            if (item.kind === "row") indexes.set(gitChangeId(item.row), index)
        })
        return indexes
    }, [virtualItems])
    const selectedIds = useMemo(() => gitChangeIdSet(selection), [selection])
    const showFilter = rows.length > FILE_FILTER_MIN_COUNT

    useEffect(() => {
        reconcileGitChangeSelection(rows)
    }, [rows, reconcileGitChangeSelection])

    const changesCount = rows.length - buckets.staged.length

    const selectedRow = selectedPath
        ? model.rowById.get(`${selectedStaged ? "s" : "c"}:${selectedPath}`)
        : undefined
    const selectedBadge = selectedRow?.badge ?? null
    const loadIdentity = repositoryRoot && selectedPath
        ? `${repositoryRoot}:${statusRevision}:${selectedStaged ? "s" : "c"}:${selectedPath}:${selectedRow?.origPath ?? ""}`
        : ""
    const diff = loadState?.identity === loadIdentity ? loadState.diff : null
    const loadError = loadState?.identity === loadIdentity ? loadState.error : null

    const selectedLang = selectedPath ? langLabel(selectedPath) : ""
    const stats = useMemo(() => (diff ? diffStats(diff) : null), [diff])

    // Clear immediately on path/side change so the previous file never flashes.
    useEffect(() => {
        if (!repositoryRoot || !selectedPath || !selectedRow) {
            setLoadState(null)
            return
        }
        const identity = loadIdentity
        setLoadState(null)
        let cancelled = false
        void loadWorktreeDiff(repositoryRoot, selectedPath, selectedStaged, selectedRow.origPath)
            .then((content) => {
                if (cancelled) return
                setLoadState({ identity, diff: content, error: null })
            })
            .catch((error: unknown) => {
                if (cancelled) return
                setLoadState({
                    identity,
                    diff: null,
                    error: error instanceof Error ? error.message : String(error)
                })
            })
        return () => {
            cancelled = true
        }
    }, [repositoryRoot, selectedPath, selectedStaged, selectedRow?.origPath, loadIdentity, retryToken])

    useLayoutEffect(() => {
        if (!filesCollapsed) return
        const panel = filePanelContentRef.current
        const expand = expandFilesRef.current
        if (!panel || !expand) return
        const active = document.activeElement
        if (active instanceof Node && panel.contains(active)) expand.focus()
    }, [filesCollapsed])

    async function stageOne(row: GitChangeRow) {
        if (!repositoryRoot) return
        const movedSides = { [row.path]: true }
        const ok = await runOp("stage", () => gitStage(repositoryRoot, [row.path]), {
            afterMutationBeforeRefresh: () => {
                useUiStore.getState().applyGitChangeMovedSides(movedSides)
            }
        })
        if (ok) void logUserAction("git_stage", `stage ${row.path}`)
    }

    async function unstageOne(row: GitChangeRow) {
        if (!repositoryRoot) return
        const movedSides = { [row.path]: false }
        const ok = await runOp("unstage", () => gitUnstage(repositoryRoot, [row.path]), {
            afterMutationBeforeRefresh: () => {
                useUiStore.getState().applyGitChangeMovedSides(movedSides)
            }
        })
        if (ok) void logUserAction("git_unstage", `unstage ${row.path}`)
    }

    async function stageAll() {
        if (!repositoryRoot) return
        const paths = uniquePaths(rows.filter((r) => !r.staged && r.classification !== "conflicted"))
        if (paths.length === 0) return
        const movedSides = Object.fromEntries(paths.map((path) => [path, true]))
        const ok = await runOp("stage", () => gitStage(repositoryRoot, paths), {
            afterMutationBeforeRefresh: () => {
                useUiStore.getState().applyGitChangeMovedSides(movedSides)
            }
        })
        if (ok) void logUserAction("git_stage", `stage all (${paths.length})`)
    }

    function toggleFilesPanel() {
        const panel = filesPanelRef.current
        if (!panel) return
        if (panel.isCollapsed()) panel.expand()
        else panel.collapse()
    }
    const focusRow = visibleOrder.find((row) => sameGitChange(row, primary)) ?? visibleOrder[0] ?? null
    const focusId = focusRow ? gitChangeId(focusRow) : null
    const activeDescendant = focusRow ? gitChangeDomId("local-file", focusRow) : undefined
    const previousFocusIdRef = useRef<string | null>(null)

    useLayoutEffect(() => {
        if (previousFocusIdRef.current === focusId) return
        previousFocusIdRef.current = focusId
        if (!focusId) return
        const index = virtualIndexById.get(focusId)
        if (index == null) return
        virtualListRef.current?.scrollToIndex(index)
    }, [focusId, virtualIndexById])

    function selectVisible(row: GitChangeRow, mode: "single" | "toggle" | "range") {
        selectGitChange(row, visibleOrder, mode)
    }

    function onListKeyDown(event: ReactKeyboardEvent, row: GitChangeRow) {
        if (event.target !== event.currentTarget) return
        const current = visibleOrder.findIndex((candidate) => sameGitChange(candidate, row))
        const nextIndex = moveListIndex(current, visibleOrder.length, event.key)
        if (nextIndex != null) {
            event.preventDefault()
            const next = visibleOrder[nextIndex]
            if (!next) return
            selectVisible(next, event.shiftKey ? "range" : "single")
            const index = virtualIndexById.get(gitChangeId(next))
            if (index != null) {
                virtualListRef.current?.scrollToIndex(index, gitChangeDomId("local-file", next))
            }
            return
        }
        if (event.key === "Enter" || event.key === " ") {
            event.preventDefault()
            selectVisible(row, event.shiftKey ? "range" : isGitToggleModifier(event) ? "toggle" : "single")
        }
    }

    const sectionLabels: Record<GitSectionKey, string> = {
        conflicts: t("localChangesTab.sectionConflicts"),
        staged: t("localChangesTab.sectionStaged"),
        unstaged: t("localChangesTab.sectionUnstaged"),
        untracked: t("localChangesTab.sectionUntracked")
    }

    function renderVirtualItem(item: GitChangeVirtualItem, _index: number, style: CSSProperties) {
        if (item.kind === "section") {
            return (
                <div key={item.key} style={style} className="flex items-center gap-[6px] px-[6px] text-[9.5px] font-semibold tracking-[0.07em] text-(--ink-3) uppercase">
                    <span>{sectionLabels[item.section]}</span>
                    <span className="font-mono text-(--ink-4)">{item.rows.length}</span>
                </div>
            )
        }
        const { row } = item
        const optionId = gitChangeDomId("local-file", row)
        return (
            <FileRow
                key={item.key}
                style={style}
                row={row}
                selected={selectedIds.has(gitChangeId(row))}
                tabbable={focusRow ? sameGitChange(focusRow, row) : false}
                optionId={optionId}
                onSelect={(event) => selectVisible(
                    row,
                    event.shiftKey ? "range" : isGitToggleModifier(event) ? "toggle" : "single"
                )}
                onContextMenu={(event) => openGitChangeContextMenu(event, row, visualOrder)}
                onStageToggle={() => void (row.staged ? unstageOne(row) : stageOne(row))}
                onListKeyDown={(event) => onListKeyDown(event, row)}
                disabled={busy != null || snapshotStale || row.classification === "conflicted"}
            />
        )
    }

    return (
        <ResizablePanelGroup
            id="local-changes-layout"
            orientation="horizontal"
            data-testid="local-changes-layout"
            data-diff-surface=""
            className="min-h-0 flex-1"
        >
            <ResizablePanel
                id="local-files"
                panelRef={filesPanelRef}
                defaultSize="34"
                minSize="15"
                maxSize="40"
                collapsible
                collapsedSize="0"
                onCollapse={() => setFilesCollapsed(true)}
                onExpand={() => setFilesCollapsed(false)}
                className="min-h-0"
                data-files-collapsed={filesCollapsed ? "true" : "false"}
            >
            <div
                ref={filePanelContentRef}
                id="local-changes-list"
                data-testid="local-changes-list"
                inert={filesCollapsed || undefined}
                aria-hidden={filesCollapsed || undefined}
                className="flex h-full min-w-0 flex-col border-r border-(--line-1) bg-(--paper-1)"
            >
                <div className="flex h-[36px] shrink-0 items-center gap-[8px] border-b border-(--line-1) px-[13px]">
                    <span className="text-[9.5px] font-semibold tracking-[0.07em] text-(--ink-3) uppercase">
                        {t("localChangesTab.label")}
                    </span>
                    <span className="flex-1" />
                    {changesCount > 0 && (
                        <Button
                            type="button"
                            variant="ghost"
                            size="xs"
                            disabled={busy != null || snapshotStale}
                            onClick={stageAll}
                            className="text-[10.5px] font-semibold text-(--yz-accent-ink)"
                        >
                            {t("localChangesTab.stageAll")}
                        </Button>
                    )}
                </div>
                {showFilter && (
                    <div className="shrink-0 px-[9px] py-[6px]">
                        <Input
                            value={fileFilter}
                            onChange={(event) => setFileFilter(event.target.value)}
                            aria-label={t("localChangesTab.filterFiles")}
                            placeholder={t("localChangesTab.filterFilesPlaceholder")}
                            className="h-[28px] text-[12px]"
                        />
                    </div>
                )}
                <VirtualizedGitChangeList
                    ref={virtualListRef}
                    items={virtualItems}
                    renderItem={renderVirtualItem}
                    className="min-h-0 flex-1"
                    viewportClassName="px-[9px] py-[7px]"
                    testId="local-changes-scroll"
                    spacerTestId="local-changes-spacer"
                    contentRole="listbox"
                    contentAriaLabel={t("localChangesTab.filesLabel")}
                    activeDescendant={activeDescendant}
                    pinnedKey={focusRow ? gitChangeId(focusRow) : undefined}
                    resetKey={fileFilter}
                />
            </div>
            </ResizablePanel>
            <ResizableHandle
                id="local-files-handle"
                data-testid="local-files-handle"
                disabled={filesCollapsed}
                aria-hidden={filesCollapsed || undefined}
                className={filesCollapsed ? "pointer-events-none w-0 overflow-hidden border-0 bg-transparent after:hidden" : undefined}
            />
            <ResizablePanel id="local-diff" defaultSize="66" minSize="60" className="min-h-0">
            <div className="relative flex h-full min-w-0 flex-1 flex-col bg-(--paper-0)">
                <div
                    data-diff-header
                    className="flex h-[38px] shrink-0 items-center gap-[10px] border-b border-(--line-1) bg-(--yz-sunk) px-[10px]"
                >
                    <DiffFilesToggle
                        ref={expandFilesRef}
                        collapsed={filesCollapsed}
                        label={filesCollapsed ? t("localChangesTab.expandFiles") : t("localChangesTab.collapseFiles")}
                        controlsId="local-changes-list"
                        onToggle={toggleFilesPanel}
                    />
                    {selectedBadge && <GitBadge badge={selectedBadge} />}
                    <span className="min-w-0 flex-1 truncate font-mono text-[12px] font-semibold text-(--ink-1)">
                        {selectedPath ?? ""}
                    </span>
                    {selectedLang && (
                        <span className="shrink-0 font-mono text-[11px] text-(--ink-3)">
                            {selectedLang}
                        </span>
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
                    <ToggleGroup
                        type="single"
                        value={diffMode}
                        onValueChange={(value) => value && setDiffMode(value as DiffMode)}
                        className="flex shrink-0 gap-[3px] rounded-[9px] bg-(--yz-sunk) p-[3px]"
                    >
                        {(["unified", "split"] as const).map((m) => (
                            <ToggleGroupItem
                                key={m}
                                value={m}
                                aria-label={m === "unified" ? t("localChangesTab.unified") : t("localChangesTab.split")}
                                className="h-[26px] rounded-[7px] px-[12px] text-[11px] font-semibold transition-all duration-[140ms] data-[state=on]:bg-(--yz-solid) data-[state=on]:text-(--ink-0) data-[state=on]:shadow-(--shadow-xs) data-[state=off]:text-(--ink-3)"
                            >
                                {m === "unified" ? t("localChangesTab.unified") : t("localChangesTab.split")}
                            </ToggleGroupItem>
                        ))}
                    </ToggleGroup>
                </div>
                <div className="min-h-0 flex-1 overflow-hidden">
                    {diff ? (
                        <DiffView content={diff} mode={diffMode} path={selectedPath ?? ""} />
                    ) : loadError ? (
                        <div className="flex h-full flex-col items-center justify-center gap-[10px] px-[16px] text-center">
                            <p role="alert" className="text-[12.5px] text-(--ink-2)">
                                {t("localChangesTab.loadFailed", { message: loadError })}
                            </p>
                            <Button
                                type="button"
                                size="xs"
                                onClick={() => setRetryToken((token) => token + 1)}
                            >
                                {t("localChangesTab.retry")}
                            </Button>
                        </div>
                    ) : (
                        <div className="flex h-full items-center justify-center text-[12.5px] text-(--ink-3)">
                            {selectedPath
                                ? t("localChangesTab.loadingEllipsis")
                                : t("localChangesTab.selectFilePrompt")}
                        </div>
                    )}
                </div>
            </div>
            </ResizablePanel>
        </ResizablePanelGroup>
    )
}

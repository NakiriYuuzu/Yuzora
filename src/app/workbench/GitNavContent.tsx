import {
    useEffect,
    useMemo,
    useRef,
    useState,
    type CSSProperties,
    type KeyboardEvent,
    type MouseEvent as ReactMouseEvent
} from "react"
import { ChevronDown, Diff, FolderGit2, GitBranch, RefreshCw, Trash2 } from "lucide-react"
import { useTranslation } from "react-i18next"

import { EmptyState } from "@/app/workbench/EmptyState"
import { Button } from "@/components/ui/button"
import { Checkbox } from "@/components/ui/checkbox"
import { Collapsible, CollapsibleTrigger } from "@/components/ui/collapsible"
import { Textarea } from "@/components/ui/textarea"
import { logUserAction } from "@/features/logs/userAction"
import { gitCommit, gitDiscard, gitStage, gitUnstage } from "@/lib/ipc"
import { isMacPlatform } from "@/lib/platform"
import { requestAppConfirmation } from "@/state/appDialogStore"
import { useDiffModalStore } from "@/state/diffModalStore"
import { useGitStore } from "@/state/gitStore"
import { useUiStore } from "@/state/uiStore"
import { useWorkspaceStore } from "@/state/workspaceStore"
import { BranchPopover } from "@/workbench/git/BranchPopover"
import { splitPath } from "@/workbench/git/diffLoad"
import { GitBadge, worktreeFilesFrom } from "@/workbench/git/fileRows"
import { openGitChangeContextMenu } from "@/workbench/git/gitChangeContextMenu"
import {
    buildGitChangeModel,
    currentGitChanges,
    gitChangeId,
    gitChangeIdSet,
    gitChangeVisibleOrder,
    isGitToggleModifier,
    type GitSectionKey,
    sectionSelectionState,
    selectedMutationSubsets,
    toggleSectionSelection,
    uniquePaths,
    type GitChangeRow
} from "@/workbench/git/gitChangeSelection"
import {
    VirtualizedGitChangeList,
    gitChangeVirtualItems,
    type GitChangeVirtualItem
} from "@/workbench/git/VirtualizedGitChangeList"

export function GitGuidedSetup({
    reason,
    kind,
    minimumVersion
}: {
    reason: string
    kind?: "notFound" | "unsupportedVersion"
    minimumVersion?: string
}) {
    const { t } = useTranslation("workbench")
    const detect = useGitStore((s) => s.detect)
    const workspacePath = useWorkspaceStore((s) => s.workspacePath)
    const showMacCommands = isMacPlatform()
    const unsupported = kind === "unsupportedVersion"
    useEffect(() => { if (reason) console.warn("git executable unavailable:", reason) }, [reason])
    return (
        <div className="flex h-full flex-col items-center justify-center gap-[14px] px-4 text-center">
            <EmptyState
                icon={GitBranch}
                title={unsupported ? t("gitSetup.unsupportedTitle") : t("gitSetup.title")}
                description={
                    unsupported
                        ? t("gitSetup.unsupportedDescription", {
                              version: minimumVersion ?? "2.24"
                          })
                        : t("gitSetup.description")
                }
            />
            <div className="max-w-[280px] rounded-[10px] border border-(--line-1) bg-(--yz-panel) px-[12px] py-[10px] text-left">
                {unsupported ? (
                    <p className="text-[11px] leading-[1.5] text-(--ink-3)">
                        {t("gitSetup.unsupportedHint", { version: minimumVersion ?? "2.24" })}
                    </p>
                ) : showMacCommands ? <>
                    <p className="text-[11px] leading-[1.5] text-(--ink-3)">{t("gitSetup.macHeading")}</p>
                    <p className="mt-[3px] font-mono text-[11px] leading-[1.5] text-(--ink-2)">{t("gitSetup.macCommandXcode")}</p>
                    <p className="mt-[2px] font-mono text-[11px] leading-[1.5] text-(--ink-2)">{t("gitSetup.macCommandBrew")}</p>
                </> : <p className="text-[11px] leading-[1.5] text-(--ink-3)">{t("gitSetup.otherHeading")}</p>}
                {!unsupported && (
                    <p className="mt-[6px] text-[11px] leading-[1.5] text-(--ink-3)">{t("gitSetup.downloadUrl")}</p>
                )}
            </div>
            <Button type="button" size="sm" onClick={() => workspacePath && void detect(workspacePath)} className="flex h-[28px] items-center gap-[6px] rounded-[8px] bg-(--yz-solid) px-[11px] text-[11.5px] font-semibold text-(--ink-0) shadow-(--shadow-xs) hover:bg-(--yz-hover)">
                <RefreshCw className="size-[12px]" aria-hidden="true" />{t("gitSetup.redetect")}
            </Button>
        </div>
    )
}

export function GitNavContent() {
    const { t } = useTranslation("menus")
    const environment = useGitStore((s) => s.environment)
    const status = useGitStore((s) => s.status)
    const lastError = useGitStore((s) => s.lastError)
    const detect = useGitStore((s) => s.detect)
    const workspacePath = useWorkspaceStore((s) => s.workspacePath)
    if (environment?.status === "missing") return <GitGuidedSetup reason={environment.reason} kind={environment.kind} minimumVersion={environment.minimumVersion} />
    if (environment?.status === "ready" && status) return <GitNavReady />
    const notARepo = environment?.status === "notARepo"
    const readyWithoutStatus = environment?.status === "ready"
    const title = readyWithoutStatus
        ? t(lastError ? "gitNav.statusErrorTitle" : "gitNav.statusLoadingTitle")
        : t(notARepo ? "gitNav.notARepoTitle" : "gitNav.noRepoTitle")
    const description = readyWithoutStatus
        ? t(lastError ? "gitNav.statusErrorDescription" : "gitNav.statusLoadingDescription", { message: lastError })
        : t(notARepo ? "gitNav.notARepoDescription" : "gitNav.noRepoDescription")
    return <div className="flex h-full flex-col items-center justify-center gap-[10px] p-[12px] text-center">
        <EmptyState icon={FolderGit2} title={title} description={description} />
        {lastError && <p role="alert" className="max-w-full break-words text-[11px] text-(--danger)">{t("gitNav.error", { message: lastError })}</p>}
        {lastError && workspacePath && <Button type="button" size="xs" onClick={() => void detect(workspacePath)}>{t("gitNav.retry")}</Button>}
    </div>
}

function GitNavReady() {
    const { t } = useTranslation("menus")
    const status = useGitStore((s) => s.status)
    const busy = useGitStore((s) => s.busy)
    const lastError = useGitStore((s) => s.lastError)
    const snapshotStale = useGitStore((s) => s.snapshotStale)
    const refresh = useGitStore((s) => s.refresh)
    const runOp = useGitStore((s) => s.runOp)
    const repositoryRoot = useGitStore((s) => s.environment?.status === "ready" ? s.environment.root : null)
    const commitMessage = useGitStore((s) => s.commitMessage)
    const setCommitMessage = useGitStore((s) => s.setCommitMessage)
    const openWorktree = useDiffModalStore((s) => s.openWorktree)
    const selection = useUiStore((s) => s.gitChangeSelection)
    const selectGitChange = useUiStore((s) => s.selectGitChange)
    const reconcileGitChangeSelection = useUiStore((s) => s.reconcileGitChangeSelection)
    const clearGitChangeSelection = useUiStore((s) => s.clearGitChangeSelection)
    const selectVisibleGitChanges = useUiStore((s) => s.selectVisibleGitChanges)
    const setGitChangeSelection = useUiStore((s) => s.setGitChangeSelection)
    const [branchOpen, setBranchOpen] = useState(false)
    const [openSections, setOpenSections] = useState<Record<GitSectionKey, boolean>>({
        conflicts: true,
        staged: true,
        unstaged: true,
        untracked: true
    })

    const branchName = status?.detached
        ? t("gitNav.detachedAt", { hash: status.headOid.slice(0, 7) })
        : (status?.branch ?? "main")
    const model = useMemo(() => buildGitChangeModel(status), [status])
    const { rows, visualOrder, buckets } = model
    const { conflicts, staged, unstaged, untracked } = buckets
    const visibleOrder = useMemo(
        () => gitChangeVisibleOrder(rows, openSections),
        [rows, openSections]
    )
    const virtualItems = useMemo(
        () => gitChangeVirtualItems(model, { openSections }),
        [model, openSections]
    )
    const working = [...unstaged, ...untracked]
    const files = useMemo(() => worktreeFilesFrom(status), [status])
    const selectedRows = currentGitChanges(selection, model.rowById)
    const selectedIds = useMemo(() => gitChangeIdSet(selection), [selection])
    const mutationSubsets = selectedMutationSubsets(selectedRows)
    const mutationsDisabled = busy != null || snapshotStale
    const canCommit = staged.length > 0 && commitMessage.trim().length > 0 && !mutationsDisabled

    useEffect(() => {
        reconcileGitChangeSelection(rows)
    }, [rows, reconcileGitChangeSelection])

    async function stageRows(targets: readonly GitChangeRow[]) {
        if (!repositoryRoot) return
        const paths = uniquePaths(targets)
        if (!paths.length) return
        const movedSides = Object.fromEntries(paths.map((path) => [path, true]))
        const ok = await runOp("stage", () => gitStage(repositoryRoot, paths), {
            afterMutationBeforeRefresh: () => {
                useUiStore.getState().applyGitChangeMovedSides(movedSides)
            }
        })
        if (ok) void logUserAction("git_stage", `stage (${paths.length})`)
    }
    async function unstageRows(targets: readonly GitChangeRow[]) {
        if (!repositoryRoot) return
        const paths = uniquePaths(targets)
        if (!paths.length) return
        const movedSides = Object.fromEntries(paths.map((path) => [path, false]))
        const ok = await runOp("unstage", () => gitUnstage(repositoryRoot, paths), {
            afterMutationBeforeRefresh: () => {
                useUiStore.getState().applyGitChangeMovedSides(movedSides)
            }
        })
        if (ok) void logUserAction("git_unstage", `unstage (${paths.length})`)
    }
    async function commit() {
        if (!canCommit) return
        const message = commitMessage.trim()
        if (!repositoryRoot) return
        const ok = await runOp("commit", () => gitCommit(repositoryRoot, message))
        if (ok) { void logUserAction("git_commit", `commit: ${message}`); setCommitMessage("") }
    }
    async function discardRows(targets: readonly GitChangeRow[], kind: "all" | "selected") {
        if (!repositoryRoot || !targets.length || mutationsDisabled) return
        const capturedRoot = repositoryRoot
        const capturedRevision = useGitStore.getState().statusRevision
        const capturedTracked = uniquePaths(targets.filter((row) => row.classification !== "untracked"))
        const capturedUntracked = uniquePaths(targets.filter((row) => row.classification === "untracked"))
        const accepted = await requestAppConfirmation({
            title: t(kind === "all" ? "gitNav.discardConfirmTitle" : "gitNav.discardSelectedConfirmTitle"),
            description: t(kind === "all" ? "gitNav.discardConfirmDescription" : "gitNav.discardSelectedConfirmDescription"),
            confirmLabel: t("gitNav.discardConfirm"),
            cancelLabel: t("gitNav.cancel"),
            kind: "warning",
            destructive: true,
        })
        if (!accepted) return
        const live = useGitStore.getState()
        const liveRoot = live.environment?.status === "ready" ? live.environment.root : null
        if (
            liveRoot !== capturedRoot
            || live.snapshotStale
            || live.busy != null
            || live.statusRevision !== capturedRevision
            || live.status == null
        ) return
        const liveDiscardable = buildGitChangeModel(live.status).visualOrder.filter((row) =>
            !row.staged && row.classification !== "conflicted"
        )
        const liveTracked = uniquePaths(liveDiscardable.filter((row) => row.classification !== "untracked" && capturedTracked.includes(row.path)))
        const liveUntracked = uniquePaths(liveDiscardable.filter((row) => row.classification === "untracked" && capturedUntracked.includes(row.path)))
        if (
            !samePathSet(liveTracked, capturedTracked)
            || !samePathSet(liveUntracked, capturedUntracked)
        ) return
        const ok = await runOp("discard", () => gitDiscard(capturedRoot, capturedTracked, capturedUntracked))
        if (ok) {
            void logUserAction("git_discard", `${kind} (${capturedTracked.length + capturedUntracked.length})`)
            reconcileGitChangeSelection(buildGitChangeModel(useGitStore.getState().status).rows)
        }
    }
    function selectRow(row: GitChangeRow, event: ReactMouseEvent | KeyboardEvent) {
        selectGitChange(row, visibleOrder, event.shiftKey ? "range" : isGitToggleModifier(event) ? "toggle" : "single")
    }
    function openRowDiff(row: GitChangeRow) {
        if (repositoryRoot) openWorktree(repositoryRoot, files, { path: row.path, staged: row.staged })
    }
    function toggleSection(section: readonly GitChangeRow[]) {
        setGitChangeSelection(toggleSectionSelection(section, selectedRows))
    }
    const sectionLabels: Record<GitSectionKey, string> = {
        conflicts: t("gitNav.sectionConflicts"),
        staged: t("gitNav.sectionStaged"),
        unstaged: t("gitNav.sectionChanges"),
        untracked: t("gitNav.sectionUntracked")
    }
    const sectionActions: Partial<Record<GitSectionKey, { label: string; run: () => void }>> = {
        staged: { label: t("gitNav.unstageAll"), run: () => void unstageRows(staged) },
        unstaged: { label: t("gitNav.stageAll"), run: () => void stageRows(unstaged) },
        untracked: { label: t("gitNav.stageAllUntracked"), run: () => void stageRows(untracked) }
    }
    function renderVirtualItem(item: GitChangeVirtualItem, _index: number, style: CSSProperties) {
        if (item.kind === "section") {
            const action = sectionActions[item.section]
            return (
                <ChangeSectionHeader
                    key={item.key}
                    style={style}
                    sectionKey={item.section}
                    open={openSections[item.section]}
                    onOpenChange={(open) => setOpenSections((current) => ({ ...current, [item.section]: open }))}
                    label={sectionLabels[item.section]}
                    rows={item.rows}
                    selectedIds={selectedIds}
                    onToggleSection={() => toggleSection(item.rows)}
                    actionLabel={action?.label}
                    disabled={mutationsDisabled}
                    onAction={action?.run}
                />
            )
        }
        const { row } = item
        return (
            <SidebarFileRow
                key={item.key}
                style={style}
                row={row}
                selected={selectedIds.has(gitChangeId(row))}
                onSelect={(event) => selectRow(row, event)}
                onToggle={() => selectGitChange(row, visibleOrder, "toggle")}
                onOpenDiff={() => openRowDiff(row)}
                onSelectAll={() => selectVisibleGitChanges(visibleOrder)}
                onContextMenu={(event) => openGitChangeContextMenu(event, row, visualOrder)}
            />
        )
    }
    const branchSecondary = status?.detached
        ? t("gitNav.detachedHead")
        : status?.upstream ?? t("gitNav.noUpstream")
    const branchTitle = [
        branchName,
        branchSecondary,
        !status?.detached && status && (status.ahead > 0 || status.behind > 0)
            ? t("gitNav.aheadBehind", { ahead: status.ahead, behind: status.behind })
            : null,
    ].filter(Boolean).join(" · ")
    const branchTrigger = (
        <Button
            type="button"
            variant="outline"
            size="sm"
            aria-busy={busy != null || undefined}
            aria-label={t("gitNav.branchesAriaLabel")}
            aria-expanded={branchOpen}
            aria-haspopup="dialog"
            title={branchTitle}
            onClick={() => setBranchOpen((open) => !open)}
            className="relative flex h-[52px] w-full min-w-0 max-w-full flex-col items-stretch justify-center gap-[2px] rounded-[10px] border border-[rgba(134,184,31,0.36)] bg-(--yz-solid) px-[9px] pr-[30px] shadow-(--shadow-xs) hover:bg-(--paper-1)"
        >
            <span className="flex min-w-0 items-center gap-[7px]">
                <span className="size-[7px] shrink-0 rounded-full bg-(--yz-accent)" aria-hidden="true" />
                <GitBranch className="size-[12px] shrink-0 text-(--ink-2)" aria-hidden="true" />
                <span className="min-w-0 truncate text-left font-mono text-[11px] font-medium text-(--ink-1)">{branchName}</span>
                <ChevronDown className={`absolute top-[10px] right-[10px] size-[12px] text-(--ink-3) transition-transform ${branchOpen ? "rotate-180" : ""}`} aria-hidden="true" />
            </span>
            <span className="flex min-w-0 items-center gap-[5px] pl-[19px] text-left font-mono text-[10px] text-(--ink-3)">
                {!status?.detached && (status?.ahead ?? 0) > 0 && <span className="shrink-0 text-[#2456cc]">↑{status?.ahead}</span>}
                {!status?.detached && (status?.behind ?? 0) > 0 && <span className="shrink-0 text-[#c8521f]">↓{status?.behind}</span>}
                <span className="min-w-0 truncate">{branchSecondary}</span>
            </span>
        </Button>
    )

    return <div data-testid="git-nav-layout" className="flex h-full min-h-0 min-w-0 flex-col overflow-hidden">
        <header data-testid="git-nav-summary" className="shrink-0 border-b border-(--line-1) p-[8px]">
            <div className="w-full min-w-0"><BranchPopover open={branchOpen} onOpenChange={setBranchOpen} trigger={branchTrigger} /></div>
            <div className="mt-[7px] flex min-w-0 flex-wrap gap-[4px]"><CountBadge label={t("gitNav.countConflicts", { count: conflicts.length })} danger hidden={!conflicts.length} /><CountBadge label={t("gitNav.countStaged", { count: staged.length })} hidden={!staged.length} /><CountBadge label={t("gitNav.countUnstaged", { count: unstaged.length })} hidden={!unstaged.length} /><CountBadge label={t("gitNav.countUntracked", { count: untracked.length })} hidden={!untracked.length} />{busy && <span className="truncate text-[10px] text-(--ink-3)">{t("gitNav.refreshing")}</span>}{!busy && snapshotStale && <span className="truncate text-[10px] text-(--ink-3)">{t("gitNav.stale")}</span>}</div>
            {lastError && <div className="mt-[6px] flex min-w-0 items-center gap-[6px]"><p role="alert" className="min-w-0 flex-1 truncate text-[10px] text-(--danger)" title={lastError}>{t("gitNav.error", { message: lastError })}</p><Button type="button" variant="ghost" size="xs" onClick={() => void refresh()} className="shrink-0 text-[10px] font-semibold text-(--yz-accent-ink)">{t("gitNav.retry")}</Button></div>}
        </header>
        {rows.length ? (
            <VirtualizedGitChangeList
                items={virtualItems}
                renderItem={renderVirtualItem}
                testId="git-nav-scroll"
                spacerTestId="git-nav-spacer"
                className="min-h-0 flex-1"
                viewportClassName="px-[5px] py-[6px]"
                onKeyDown={(event) => {
                    if ((event.metaKey || event.ctrlKey) && event.key.toLowerCase() === "a") {
                        event.preventDefault()
                        selectVisibleGitChanges(visibleOrder)
                    }
                }}
            />
        ) : (
            <div data-testid="git-nav-scroll" className="min-h-0 flex-1 px-[8px] py-[24px] text-center">
                <p className="text-[12px] font-semibold text-(--ink-2)">{t("gitNav.cleanTitle")}</p>
                <p className="mt-[4px] text-[10.5px] leading-[1.45] text-(--ink-4)">{t("gitNav.cleanDescription")}</p>
            </div>
        )}
        {selectedRows.length > 0 && (
            <div data-testid="git-nav-bulk" className="flex min-w-0 shrink-0 flex-wrap items-center gap-[6px] border-t border-(--line-1) px-[8px] py-[6px]">
                <span className="min-w-0 flex-1 truncate text-[10.5px] font-medium text-(--ink-2)">
                    {t("gitNav.selectedCount", { count: selectedRows.length })}
                    {mutationSubsets.conflicts.length > 0 && (
                        <span className="ml-[6px] text-(--ink-4)">{t("gitNav.conflictsExcluded", { count: mutationSubsets.conflicts.length })}</span>
                    )}
                </span>
                {mutationSubsets.stageable.length > 0 && (
                    <Button type="button" variant="ghost" size="xs" disabled={mutationsDisabled} onClick={() => void stageRows(mutationSubsets.stageable)} className="shrink-0 text-[10.5px] font-semibold text-(--yz-accent-ink)">{t("gitNav.bulkStage")}</Button>
                )}
                {mutationSubsets.unstageable.length > 0 && (
                    <Button type="button" variant="ghost" size="xs" disabled={mutationsDisabled} onClick={() => void unstageRows(mutationSubsets.unstageable)} className="shrink-0 text-[10.5px] font-semibold text-(--yz-accent-ink)">{t("gitNav.bulkUnstage")}</Button>
                )}
                {mutationSubsets.discardable.length > 0 && (
                    <Button type="button" variant="ghost" size="xs" disabled={mutationsDisabled} onClick={() => void discardRows(mutationSubsets.discardable, "selected")} className="shrink-0 text-[10.5px] font-semibold text-[#c2293f]">{t("gitNav.bulkDiscard")}</Button>
                )}
                <Button type="button" variant="ghost" size="xs" onClick={clearGitChangeSelection} className="shrink-0 text-[10.5px]">{t("gitNav.clearSelection")}</Button>
            </div>
        )}
        <footer data-testid="git-nav-composer" className="shrink-0 border-t border-(--line-1) p-[8px]">
            <label htmlFor="git-commit-message" className="mb-[4px] block text-[9.5px] font-semibold uppercase tracking-[0.06em] text-(--ink-3)">{t("gitNav.commitLabel")}</label>
            <Textarea id="git-commit-message" value={commitMessage} disabled={mutationsDisabled} onChange={(event) => setCommitMessage(event.target.value)} onKeyDown={(event) => { if ((event.metaKey || event.ctrlKey) && event.key === "Enter") { event.preventDefault(); void commit() } }} placeholder={t("gitNav.commitPlaceholder")} rows={2} className="min-h-[48px] w-full resize-none rounded-[8px] border border-(--line-1) bg-(--yz-solid) px-[8px] py-[6px] text-[11.5px] text-(--ink-1) outline-none focus:border-(--ink-3)" />
            <div className="mt-[6px] flex min-w-0 gap-[5px]"><Button type="button" aria-label={t("gitNav.commit")} disabled={!canCommit} onClick={() => void commit()} className="min-w-0 flex-1 truncate rounded-[8px] bg-(--ink-1) px-[7px] py-[6px] text-[11px] font-semibold text-(--paper-0) disabled:cursor-not-allowed disabled:bg-(--paper-3) disabled:text-(--ink-4)">{t("gitNav.commit")} {staged.length || ""}</Button><Button type="button" variant="outline" aria-label={t("gitNav.reviewDiffAria")} title={t("gitNav.reviewDiffAria")} disabled={!files.length} onClick={() => repositoryRoot && openWorktree(repositoryRoot, files)} className="shrink-0 rounded-[8px] border border-(--line-1) bg-(--yz-solid) px-[8px] py-[6px] text-[11px] font-semibold text-(--ink-1) disabled:opacity-50">{t("gitNav.reviewDiff")}</Button><Button type="button" variant="destructive" size="icon-sm" aria-label={t("gitNav.discardAll")} title={t("gitNav.discardAll")} disabled={!working.length || mutationsDisabled} onClick={() => void discardRows(working, "all")} className="flex size-[29px] shrink-0 items-center justify-center rounded-[8px] border border-[rgba(226,59,84,0.38)] text-[#c2293f] hover:bg-(--danger-soft) disabled:opacity-50"><Trash2 className="size-[13px]" aria-hidden="true" /></Button></div>
        </footer>
    </div>
}

function samePathSet(left: readonly string[], right: readonly string[]): boolean {
    if (left.length !== right.length) return false
    const seen = new Set(left)
    return right.every((path) => seen.has(path))
}

function CountBadge({ label, danger = false, hidden = false }: { label: string; danger?: boolean; hidden?: boolean }) { if (hidden) return null; return <span className={`max-w-full truncate rounded-(--r-pill) px-[6px] py-[1px] font-mono text-[9.5px] ${danger ? "bg-(--danger-soft) text-[#c2293f]" : "bg-(--paper-2) text-(--ink-3)"}`}>{label}</span> }

function ChangeSectionHeader({
    sectionKey,
    open,
    onOpenChange,
    label,
    rows,
    selectedIds,
    onToggleSection,
    actionLabel,
    onAction,
    disabled = false,
    style
}: {
    sectionKey: GitSectionKey
    open: boolean
    onOpenChange: (open: boolean) => void
    label: string
    rows: readonly GitChangeRow[]
    selectedIds: ReadonlySet<ReturnType<typeof gitChangeId>>
    onToggleSection: () => void
    actionLabel?: string
    onAction?: () => void
    disabled?: boolean
    style: CSSProperties
}) {
    const { t } = useTranslation("menus")
    const tri = sectionSelectionState(rows, selectedIds)
    return (
        <Collapsible style={style} open={open} onOpenChange={onOpenChange} className="group/section">
            <div className="flex h-[30px] min-w-0 items-center gap-[3px] px-[3px]">
                <Checkbox
                    checked={tri === "checked" ? true : tri === "mixed" ? "indeterminate" : false}
                    aria-label={t("gitNav.selectSectionAria", { section: label })}
                    onCheckedChange={onToggleSection}
                    className="size-[14px]"
                />
                <CollapsibleTrigger data-testid={`git-section-toggle-${sectionKey}`} className="flex min-w-0 flex-1 items-center gap-[4px] rounded-[6px] px-[3px] py-[3px] text-left hover:bg-(--yz-panel)">
                    <ChevronDown className="size-[11px] shrink-0 transition-transform group-data-[state=closed]/section:-rotate-90" aria-hidden="true" />
                    <span className="min-w-0 truncate text-[9.5px] font-semibold uppercase tracking-[0.06em] text-(--ink-3)">{label}</span>
                    <span className="shrink-0 font-mono text-[9.5px] text-(--ink-4)">{rows.length}</span>
                </CollapsibleTrigger>
                {actionLabel && <Button type="button" variant="ghost" size="xs" disabled={disabled} onClick={onAction} className="max-w-[72px] shrink-0 truncate px-[3px] text-[9.5px] font-semibold text-(--yz-accent-ink)" title={actionLabel}>{actionLabel}</Button>}
            </div>
        </Collapsible>
    )
}

function SidebarFileRow({
    row,
    selected,
    onSelect,
    onToggle,
    onOpenDiff,
    onSelectAll,
    onContextMenu,
    style
}: {
    row: GitChangeRow
    selected: boolean
    onSelect: (event: ReactMouseEvent | KeyboardEvent) => void
    onToggle: () => void
    onOpenDiff: () => void
    onSelectAll: () => void
    onContextMenu: (event: ReactMouseEvent) => void
    style: CSSProperties
}) {
    const { t } = useTranslation("menus")
    const { name, dir } = splitPath(row.path)
    const rowButtonRef = useRef<HTMLButtonElement>(null)
    return (
        <div
            style={style}
            data-selected={selected ? "true" : undefined}
            className={`group/row flex h-[32px] min-w-0 items-center gap-[6px] rounded-[7px] px-[6px] ${selected ? "bg-(--yz-active) shadow-(--shadow-xs)" : "hover:bg-(--yz-panel)"}`}
        >
            <span className="relative size-[18px] shrink-0">
                <span className={`absolute inset-0 ${selected ? "opacity-0" : "opacity-100 group-hover/row:opacity-0 group-focus-within/row:opacity-0"}`}>
                    <GitBadge badge={row.badge} />
                </span>
                <Checkbox
                    checked={selected}
                    tabIndex={-1}
                    aria-hidden="true"
                    onClick={(event) => event.stopPropagation()}
                    onPointerDown={(event) => {
                        event.preventDefault()
                        event.stopPropagation()
                    }}
                    onCheckedChange={() => {
                        onToggle()
                        rowButtonRef.current?.focus()
                    }}
                    className={`absolute inset-0 size-[18px] ${selected ? "opacity-100" : "opacity-0 group-hover/row:opacity-100 group-focus-within/row:opacity-100"}`}
                />
            </span>
            <button
                ref={rowButtonRef}
                type="button"
                aria-pressed={selected}
                title={row.path}
                onClick={onSelect}
                onDoubleClick={onOpenDiff}
                onKeyDown={(event) => {
                    if (event.key === " ") {
                        event.preventDefault()
                        onToggle()
                        return
                    }
                    if (event.key === "Enter") {
                        event.preventDefault()
                        onOpenDiff()
                        return
                    }
                    if ((event.metaKey || event.ctrlKey) && event.key.toLowerCase() === "a") {
                        event.preventDefault()
                        onSelectAll()
                    }
                }}
                onContextMenu={onContextMenu}
                className="flex min-w-0 flex-1 items-center gap-[6px] rounded-[5px] text-left outline-none focus-visible:ring-2 focus-visible:ring-(--yz-accent)"
            >
                <span className="min-w-0 flex-1 truncate text-[11.5px] font-medium text-(--ink-1)">{name}</span>
                {dir && <span className="min-w-0 max-w-[42%] truncate text-[9.5px] text-(--ink-4)" title={dir}>{dir}</span>}
            </button>
            <Button
                type="button"
                variant="ghost"
                size="icon-xs"
                aria-label={t("gitNav.openDiffAria", { path: row.path })}
                title={t("gitNav.openDiffTitle")}
                onClick={onOpenDiff}
                className="size-[24px] shrink-0 opacity-0 group-hover/row:opacity-100 group-focus-within/row:opacity-100 focus-visible:opacity-100"
            >
                <Diff aria-hidden="true" />
            </Button>
        </div>
    )
}

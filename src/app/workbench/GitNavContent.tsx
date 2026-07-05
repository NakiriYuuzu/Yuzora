import { useRef, useState } from "react"
import { FolderGit2, GitBranch, RefreshCw } from "lucide-react"

import { EmptyState } from "@/app/workbench/EmptyState"
import { gitCommit, gitDiscard, gitStage, gitUnstage, logUserAction } from "@/lib/ipc"
import type { GitStatus } from "@/lib/types"
import { useDiffModalStore } from "@/state/diffModalStore"
import { changedPathSet, useGitStore } from "@/state/gitStore"
import { useUiStore } from "@/state/uiStore"
import { useWorkspaceStore } from "@/state/workspaceStore"
import { BranchPopover } from "@/workbench/git/BranchPopover"
import { GitBadge, SectionHeader, badgeChar, worktreeFilesFrom } from "@/workbench/git/fileRows"
import { splitPath } from "@/workbench/git/diffLoad"

// Hold-to-discard timing — copied from the design prototype (dc.html L1893:
// setTimeout 760ms; L4086: fill grows `width 760ms linear` while discarding and
// resets `140ms var(--ease-out)` on release). No second confirm dialog: holding
// is the confirmation (brief E1 §4).
const HOLD_DISCARD_MS = 760

/**
 * Guided setup shown when the git executable is missing — extends EmptyState
 * with install guidance (macOS) and a re-detect button. Shared by GitNavContent
 * and GitPanel so both surfaces present the same recovery path. No upstream
 * design; extends the EmptyState visual language.
 */
export function GitGuidedSetup({ reason }: { reason: string }) {
    const detect = useGitStore((s) => s.detect)
    const workspacePath = useWorkspaceStore((s) => s.workspacePath)

    const redetect = () => {
        if (workspacePath) void detect(workspacePath)
    }

    return (
        <div className="flex h-full flex-col items-center justify-center gap-[14px] px-4 text-center">
            <EmptyState icon={GitBranch} title="未偵測到 Git" description={reason} />
            <div className="max-w-[280px] rounded-[10px] border border-(--line-1) bg-(--yz-panel) px-[12px] py-[10px] text-left">
                <p className="text-[11px] leading-[1.5] text-(--ink-3)">在 macOS 安裝 Git：</p>
                <p className="mt-[3px] font-mono text-[11px] leading-[1.5] text-(--ink-2)">
                    xcode-select --install
                </p>
                <p className="mt-[2px] font-mono text-[11px] leading-[1.5] text-(--ink-2)">
                    brew install git
                </p>
                <p className="mt-[6px] text-[11px] leading-[1.5] text-(--ink-3)">
                    https://git-scm.com/downloads
                </p>
            </div>
            <button
                type="button"
                onClick={redetect}
                className="flex h-[28px] items-center gap-[6px] rounded-[8px] bg-(--yz-solid) px-[11px] text-[11.5px] font-semibold text-(--ink-0) shadow-(--shadow-xs) transition-colors hover:bg-(--yz-hover)"
            >
                <RefreshCw className="size-[12px]" aria-hidden="true" />
                重新偵測
            </button>
        </div>
    )
}

/**
 * Git mode nav content — design reference §1 (dc.html L177-243). When a repo is
 * ready it shows the commit work card (branch pill / ahead-behind / changed pill
 * / commit message + Commit / Review diff / Hold-to-discard) followed by STAGED
 * and CHANGED per-file lists. Clicking a file row opens the Diff modal on that
 * file (openDiffWork semantics). Non-repo keeps the empty state; git-missing
 * keeps the guided setup.
 */
export function GitNavContent() {
    const environment = useGitStore((s) => s.environment)

    if (environment?.status === "missing") {
        return <GitGuidedSetup reason={environment.reason} />
    }

    if (!environment || environment.status !== "ready") {
        return (
            <div className="flex h-full items-center justify-center">
                <EmptyState
                    icon={FolderGit2}
                    title="No repository status"
                    description="Open a project to see changes here"
                />
            </div>
        )
    }

    return <GitNavReady />
}

// A flattened row from the four GitStatus buckets. `staged` picks which side the
// row belongs to so its stage/unstage action targets the right git side.
interface Row {
    path: string
    badge: string
    staged: boolean
    untracked: boolean
}

function stagedRowsFrom(status: GitStatus | null): Row[] {
    return (status?.staged ?? []).map((e) => ({
        path: e.path,
        badge: badgeChar(e.status),
        staged: true,
        untracked: false
    }))
}

// CHANGED bucket = unstaged + untracked + conflicted (same basis as
// changedPathSet). Conflicted shows "!", untracked "?".
function changedRowsFrom(status: GitStatus | null): Row[] {
    return [
        ...(status?.unstaged ?? []).map((e) => ({
            path: e.path,
            badge: badgeChar(e.status),
            staged: false,
            untracked: false
        })),
        ...(status?.untracked ?? []).map((path) => ({
            path,
            badge: "?",
            staged: false,
            untracked: true
        })),
        ...(status?.conflicted ?? []).map((e) => ({
            path: e.path,
            badge: "!",
            staged: false,
            untracked: false
        }))
    ]
}

function GitNavReady() {
    const status = useGitStore((s) => s.status)
    const runOp = useGitStore((s) => s.runOp)
    const commitMessage = useGitStore((s) => s.commitMessage)
    const setCommitMessage = useGitStore((s) => s.setCommitMessage)
    const openWorktree = useDiffModalStore((s) => s.openWorktree)

    const [branchOpen, setBranchOpen] = useState(false)

    const branchName = status?.detached
        ? status.headOid.slice(0, 7)
        : (status?.branch ?? "main")
    const ahead = status?.ahead ?? 0
    const behind = status?.behind ?? 0
    const changedCount = changedPathSet(status).size

    const stagedRows = stagedRowsFrom(status)
    const changedRows = changedRowsFrom(status)
    const stagedCount = stagedRows.length
    const canCommit = stagedCount > 0 && commitMessage.trim().length > 0

    const files = worktreeFilesFrom(status)

    async function stageOne(row: Row) {
        const ok = await runOp("stage", () => gitStage([row.path]))
        if (ok) {
            void logUserAction("git_stage", `stage ${row.path}`)
            // Keep the Local-changes selection following this file across sections
            // so the diff re-resolves against the side it now lives on (T15).
            const ui = useUiStore.getState()
            if (ui.gitSelectedPath === row.path) ui.selectGitFile(row.path, true)
        }
    }
    async function unstageOne(row: Row) {
        const ok = await runOp("unstage", () => gitUnstage([row.path]))
        if (ok) {
            void logUserAction("git_unstage", `unstage ${row.path}`)
            const ui = useUiStore.getState()
            if (ui.gitSelectedPath === row.path) ui.selectGitFile(row.path, false)
        }
    }
    async function stageAll() {
        const paths = changedRows.map((r) => r.path)
        if (paths.length === 0) return
        const ok = await runOp("stage", () => gitStage(paths))
        if (ok) void logUserAction("git_stage", `stage all (${paths.length})`)
    }
    async function unstageAll() {
        const paths = stagedRows.map((r) => r.path)
        if (paths.length === 0) return
        const ok = await runOp("unstage", () => gitUnstage(paths))
        if (ok) void logUserAction("git_unstage", `unstage all (${paths.length})`)
    }
    async function discardAll() {
        if (changedCount === 0) return
        // Exclude conflicted files: `git restore`/clean on an unmerged path errors
        // out mid-merge, surfacing spurious failures. Discard only the resolvable
        // working changes (m7).
        const conflicted = new Set((status?.conflicted ?? []).map((e) => e.path))
        const discardable = changedRows.filter((r) => !conflicted.has(r.path))
        const tracked = discardable.filter((r) => !r.untracked).map((r) => r.path)
        const untracked = discardable.filter((r) => r.untracked).map((r) => r.path)
        const done = await runOp("discard", () => gitDiscard(tracked, untracked))
        if (done) void logUserAction("git_discard", `discard all (${changedCount})`)
    }
    async function commit() {
        if (!canCommit) return
        const done = await runOp("commit", () => gitCommit(commitMessage.trim()))
        if (done) {
            void logUserAction("git_commit", `commit: ${commitMessage.trim()}`)
            setCommitMessage("")
        }
    }

    // §1.1 branch pill — trigger injected into the shared BranchPopover (h26,
    // dot + git-graph icon + mono name). Design L181-184.
    const branchPill = (
        <button
            type="button"
            aria-label="Branches"
            onClick={() => setBranchOpen((v) => !v)}
            className="flex h-[26px] shrink-0 cursor-pointer items-center gap-[7px] rounded-(--r-pill) border border-(--line-1) bg-(--yz-solid) py-0 pr-[11px] pl-[10px] shadow-(--shadow-xs) transition-colors hover:bg-(--paper-1)"
        >
            <span aria-hidden="true" className="size-[8px] shrink-0 rounded-full bg-(--yz-accent)" />
            <svg
                width="12"
                height="12"
                viewBox="0 0 24 24"
                fill="none"
                stroke="var(--ink-2)"
                strokeWidth="2"
                strokeLinecap="round"
                strokeLinejoin="round"
                aria-hidden="true"
            >
                <circle cx="6" cy="6" r="2.4" />
                <circle cx="6" cy="18" r="2.4" />
                <circle cx="18" cy="8" r="2.4" />
                <path d="M6 8.4v7.2M18 10.4a6 6 0 0 1-6 6H8.4" />
            </svg>
            <span className="font-mono text-[11.5px] font-medium text-(--ink-1)">{branchName}</span>
        </button>
    )

    return (
        <div className="flex min-h-0 flex-col">
            {/* §1 commit work card (dc.html L179-210) */}
            <div className="mx-[4px] mt-[6px] mb-[11px] shrink-0 rounded-[12px] border border-(--line-1) bg-(--yz-panel) p-[11px]">
                {/* §1.1 first row: branch pill + ahead/behind + changed pill */}
                <div className="flex flex-wrap items-center gap-[7px]">
                    <BranchPopover open={branchOpen} onOpenChange={setBranchOpen} trigger={branchPill} />
                    {(ahead > 0 || behind > 0) && (
                        <div className="flex items-center gap-[6px] font-mono text-[11.5px]">
                            {ahead > 0 && <span style={{ color: "#2456cc" }}>↑{ahead}</span>}
                            {behind > 0 && <span style={{ color: "#c8521f" }}>↓{behind}</span>}
                        </div>
                    )}
                    <div className="flex-1" />
                    {changedCount > 0 && (
                        <div
                            title={`${changedCount} files changed`}
                            className="flex h-[22px] items-center gap-[6px] rounded-(--r-pill) bg-(--amber-soft) px-[9px] text-[11.5px] font-semibold"
                            style={{ color: "#9a6512" }}
                        >
                            <span
                                aria-hidden="true"
                                className="size-[6px] rounded-[2px]"
                                style={{ background: "#d68a0c" }}
                            />
                            {changedCount} changed
                        </div>
                    )}
                </div>

                {/* §1.2 divider */}
                <div className="mt-[11px] h-px bg-(--line-1)" />

                {/* §1.3 commit message — draft in gitStore */}
                <input
                    value={commitMessage}
                    onChange={(e) => setCommitMessage(e.target.value)}
                    placeholder="Commit message…"
                    className="mt-[11px] h-[32px] w-full rounded-[9px] border border-(--line-1) bg-(--yz-solid) px-[11px] font-sans text-[12px] text-(--ink-1) outline-none focus:border-(--ink-3)"
                />

                {/* §1.4 button row: Commit / Review diff / Hold-to-discard */}
                <div className="mt-[8px] flex items-center gap-[7px]">
                    <button
                        type="button"
                        aria-label="Commit"
                        onClick={commit}
                        disabled={!canCommit}
                        className={
                            "flex h-[32px] flex-1 items-center justify-center gap-[6px] rounded-[9px] text-[11.5px] font-semibold transition-all duration-[140ms] " +
                            (canCommit
                                ? "cursor-pointer bg-(--ink-1) text-(--paper-0) shadow-(--shadow-xs)"
                                : "cursor-not-allowed bg-(--paper-3) text-(--ink-4)")
                        }
                    >
                        <svg
                            width="13"
                            height="13"
                            viewBox="0 0 24 24"
                            fill="none"
                            stroke="currentColor"
                            strokeWidth="2"
                            strokeLinecap="round"
                            strokeLinejoin="round"
                            aria-hidden="true"
                        >
                            <circle cx="12" cy="12" r="3" />
                            <path d="M3 12h6M15 12h6" />
                        </svg>
                        <span>Commit</span>
                        {stagedCount > 0 && (
                            <span aria-hidden="true" className="font-mono text-[10px]">
                                {stagedCount}
                            </span>
                        )}
                    </button>

                    <button
                        type="button"
                        title="Review diff"
                        aria-label="Review diff"
                        disabled={changedCount === 0 && stagedCount === 0}
                        onClick={() => openWorktree(files)}
                        className="flex h-[32px] shrink-0 items-center justify-center gap-[6px] rounded-[9px] border border-(--line-1) bg-(--yz-solid) px-[12px] text-[11.5px] font-semibold text-(--ink-1) shadow-(--shadow-xs) transition-colors hover:bg-(--paper-1) disabled:cursor-not-allowed disabled:opacity-50"
                    >
                        <svg
                            width="12"
                            height="12"
                            viewBox="0 0 24 24"
                            fill="none"
                            stroke="currentColor"
                            strokeWidth="2"
                            strokeLinecap="round"
                            strokeLinejoin="round"
                            aria-hidden="true"
                        >
                            <path d="M21 12a2 2 0 0 1-2 2H7l-4 4V6a2 2 0 0 1 2-2h6" />
                            <path d="M16 3h6v6" />
                            <path d="m21 3-7 7" />
                        </svg>
                        Review diff
                    </button>

                    <HoldDiscardButton
                        onDiscard={discardAll}
                        disabled={changedCount === 0}
                    />
                </div>
            </div>

            {/* §1.5 STAGED section */}
            <SectionHeader label="Staged" count={stagedCount} className="px-[6px] pt-[7px] pb-[5px]">
                <button
                    type="button"
                    onClick={unstageAll}
                    className="text-[10.5px] font-semibold text-(--yz-accent-ink)"
                >
                    Unstage all
                </button>
            </SectionHeader>
            {stagedRows.map((row) => (
                <SidebarFileRow
                    key={`s:${row.path}`}
                    row={row}
                    onSelect={() => openWorktree(files, { path: row.path, staged: row.staged })}
                    onStageToggle={() => void unstageOne(row)}
                />
            ))}

            {/* §1.6 CHANGED section */}
            <SectionHeader
                label="Changed"
                count={changedCount}
                className="px-[6px] pt-[11px] pb-[5px]"
            >
                <button
                    type="button"
                    onClick={stageAll}
                    className="text-[10.5px] font-semibold text-(--yz-accent-ink)"
                >
                    Stage all
                </button>
            </SectionHeader>
            {changedRows.map((row) => (
                <SidebarFileRow
                    key={`c:${row.path}`}
                    row={row}
                    onSelect={() => openWorktree(files, { path: row.path, staged: row.staged })}
                    onStageToggle={() => void stageOne(row)}
                />
            ))}
        </div>
    )
}

// §1.5/§1.6 sidebar file row — badge + name/dir + stage/unstage icon (row click
// opens the Diff modal; the icon button toggles staging). Design L215-222. The
// per-line +add/−del stats the design shows are deferred (no numstat pipeline —
// see brief 已知偏差).
function SidebarFileRow({
    row,
    onSelect,
    onStageToggle
}: {
    row: Row
    onSelect: () => void
    onStageToggle: () => void
}) {
    const { name, dir } = splitPath(row.path)
    return (
        <div
            role="button"
            tabIndex={0}
            onClick={onSelect}
            title="Open diff"
            className="group mx-[3px] flex h-[30px] cursor-pointer items-center gap-[9px] rounded-[8px] px-[8px] transition-[background] duration-[120ms] hover:bg-(--yz-panel)"
        >
            <GitBadge badge={row.badge} />
            <span className="flex min-w-0 flex-1 items-baseline overflow-hidden">
                <span className="truncate text-[12.5px] font-medium text-(--ink-1)">{name}</span>
                {dir && (
                    <span className="ml-[6px] shrink-0 text-[10.5px] text-(--ink-4)">{dir}</span>
                )}
            </span>
            <button
                type="button"
                aria-label={`${row.staged ? "Unstage" : "Stage"} ${row.path}`}
                title={row.staged ? "Unstage file" : "Stage file"}
                onClick={(e) => {
                    e.stopPropagation()
                    onStageToggle()
                }}
                className="flex size-[22px] shrink-0 items-center justify-center rounded-[6px] text-(--ink-3) transition-all duration-[130ms] hover:bg-(--yz-hover) hover:text-(--yz-accent-ink)"
            >
                <svg
                    width="13"
                    height="13"
                    viewBox="0 0 24 24"
                    fill="none"
                    stroke="currentColor"
                    strokeWidth="2"
                    strokeLinecap="round"
                    strokeLinejoin="round"
                    aria-hidden="true"
                >
                    {row.staged ? <path d="M5 12h14" /> : <path d="M12 5v14M5 12h14" />}
                </svg>
            </button>
        </div>
    )
}

// §1.4 hold-to-discard — press and hold HOLD_DISCARD_MS to discard all working
// changes; a fill sweeps left-to-right while held and resets on release. No
// confirm dialog (the hold is the confirmation). Design L205-208 / L1888-1895.
function HoldDiscardButton({
    onDiscard,
    disabled
}: {
    onDiscard: () => void | Promise<void>
    disabled: boolean
}) {
    const [holding, setHolding] = useState(false)
    const timer = useRef<ReturnType<typeof setTimeout> | null>(null)

    function clear() {
        if (timer.current) {
            clearTimeout(timer.current)
            timer.current = null
        }
    }

    function start() {
        if (disabled) return
        setHolding(true)
        clear()
        timer.current = setTimeout(() => {
            timer.current = null
            setHolding(false)
            void onDiscard()
        }, HOLD_DISCARD_MS)
    }

    function end() {
        clear()
        setHolding(false)
    }

    return (
        <button
            type="button"
            title="Discard all working changes"
            aria-label="Discard all working changes"
            disabled={disabled}
            onPointerDown={start}
            onPointerUp={end}
            onPointerLeave={end}
            className="relative flex size-[32px] shrink-0 items-center justify-center overflow-hidden rounded-[9px] border transition-all duration-[140ms] hover:bg-(--danger-soft) disabled:cursor-not-allowed disabled:opacity-50"
            style={{ borderColor: "rgba(226,59,84,0.38)", color: "#c2293f", background: "transparent" }}
        >
            <span
                aria-hidden="true"
                className="pointer-events-none absolute top-0 bottom-0 left-0 rounded-[9px]"
                style={{
                    background: "rgba(226,59,84,0.35)",
                    width: holding ? "100%" : "0",
                    transition: holding
                        ? `width ${HOLD_DISCARD_MS}ms linear`
                        : "width 140ms var(--ease-out)"
                }}
            />
            <svg
                width="13"
                height="13"
                viewBox="0 0 24 24"
                fill="none"
                stroke="currentColor"
                strokeWidth="2"
                strokeLinecap="round"
                strokeLinejoin="round"
                aria-hidden="true"
                style={{ position: "relative" }}
            >
                <rect x="4" y="11" width="16" height="9" rx="2" />
                <path d="M8 11V7a4 4 0 0 1 8 0v4" />
            </svg>
        </button>
    )
}

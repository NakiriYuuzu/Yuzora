import { useEffect, useMemo, useState } from "react"

import { gitStage, gitUnstage } from "../../lib/ipc"
import { logUserAction } from "@/features/logs/userAction"
import type { DiffContent } from "../../lib/types"
import { useGitStore } from "../../state/gitStore"
import { useUiStore } from "../../state/uiStore"
import { diffStats, langLabel, loadWorktreeDiff, splitPath } from "./diffLoad"
import { DiffView } from "./DiffView"
import { GitBadge, badgeChar } from "./fileRows"

type DiffMode = "unified" | "split"

// A single row's data, flattened from the four GitStatus buckets. `staged`
// records which side (staged↔changes) the row belongs to so a click selects
// the right diff and the per-row action targets the correct git side.
interface Row {
    path: string
    badge: string
    staged: boolean
}

// §2.5 flat local-changes file row (dc.html L881-887): badge + name/dir + a
// hover-revealed stage/unstage icon. Row click selects the file into the right
// column's diff; the icon toggles staging without selecting. Discard-all + the
// commit box moved to the sidebar (D1=a), so no per-row discard here.
function FileRow({
    row,
    selected,
    onSelect,
    onStageToggle
}: {
    row: Row
    selected: boolean
    onSelect: () => void
    onStageToggle: () => void
}) {
    const { name, dir } = splitPath(row.path)
    return (
        <div
            role="button"
            tabIndex={0}
            onClick={onSelect}
            className={
                "group flex h-[32px] items-center gap-[9px] rounded-[8px] px-[8px] my-[1px] cursor-pointer transition-[background] duration-[120ms] " +
                (selected
                    ? "bg-(--yz-active) shadow-(--shadow-xs)"
                    : "hover:bg-(--yz-panel)")
            }
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
                    <span className="ml-[6px] shrink-0 text-[10px] font-normal text-(--ink-4)">{dir}</span>
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
            </button>
        </div>
    )
}

/**
 * GitPanel → Local changes tab. Left column (§2.5, 312px): a single flat list of
 * all changed files (staged first), each with a stage/unstage toggle; a header
 * with a Stage all action. The commit box + discard live in the sidebar now
 * (D1=a). Right column: the selected file's diff via §4.2 diffMode toggle +
 * DiffView. Mutations go through useGitStore.runOp + logUserAction on success.
 */
export function LocalChangesTab() {
    const status = useGitStore((s) => s.status)
    const runOp = useGitStore((s) => s.runOp)
    const selectedPath = useUiStore((s) => s.gitSelectedPath)
    const selectedStaged = useUiStore((s) => s.gitSelectedStaged)
    const selectGitFile = useUiStore((s) => s.selectGitFile)

    const [diffMode, setDiffMode] = useState<DiffMode>("unified")
    const [diff, setDiff] = useState<DiffContent | null>(null)

    // §2.5 flat list — staged first, then unstaged/untracked/conflicted
    // (staged.concat(changed) ordering). `staged` picks the diff side and the
    // stage/unstage direction. Untracked → "?", conflicted → "!".
    const rows: Row[] = [
        ...(status?.staged ?? []).map((e) => ({
            path: e.path,
            badge: badgeChar(e.status),
            staged: true
        })),
        ...(status?.unstaged ?? []).map((e) => ({
            path: e.path,
            badge: badgeChar(e.status),
            staged: false
        })),
        ...(status?.untracked ?? []).map((path) => ({ path, badge: "?", staged: false })),
        ...(status?.conflicted ?? []).map((e) => ({ path: e.path, badge: "!", staged: false }))
    ]

    const changesCount = rows.filter((r) => !r.staged).length

    // §2.5 L893 diff header includes the file's status badge. Resolve it from the
    // selected side; if the file is gone (e.g. after a refresh) → null so no
    // stale badge renders (defensive).
    const selectedRow = selectedPath
        ? rows.find((r) => r.path === selectedPath && r.staged === selectedStaged)
        : undefined
    const selectedBadge = selectedRow?.badge ?? null

    // §2.5 L897-899 header: language label + per-line add/delete counts. lang is
    // pure path → string; stats need the diff content and only recompute when the
    // selection or the loaded diff changes. Undisplayable sides (tooLarge/binary)
    // yield null → no +N/−N.
    const selectedLang = selectedPath ? langLabel(selectedPath) : ""
    const stats = useMemo(() => (diff ? diffStats(diff) : null), [diff])

    // Load the diff for the current selection. Skip when nothing is selected or
    // the selected path is no longer present on its side (e.g. after staging).
    useEffect(() => {
        if (!selectedPath) {
            setDiff(null)
            return
        }
        let cancelled = false
        void loadWorktreeDiff(selectedPath, selectedStaged).then((content) => {
            if (!cancelled) setDiff(content)
        })
        return () => {
            cancelled = true
        }
    }, [selectedPath, selectedStaged])

    async function stageOne(row: Row) {
        const ok = await runOp("stage", () => gitStage([row.path]))
        if (ok) void logUserAction("git_stage", `stage ${row.path}`)
    }

    async function unstageOne(row: Row) {
        const ok = await runOp("unstage", () => gitUnstage([row.path]))
        if (ok) void logUserAction("git_unstage", `unstage ${row.path}`)
    }

    async function stageAll() {
        const paths = rows.filter((r) => !r.staged).map((r) => r.path)
        if (paths.length === 0) return
        const ok = await runOp("stage", () => gitStage(paths))
        if (ok) void logUserAction("git_stage", `stage all (${paths.length})`)
    }

    return (
        <div className="flex min-h-0 flex-1">
            {/* §2.5 left column: 312px, border-right --line-1, bg --paper-1 */}
            <div className="flex w-[312px] shrink-0 flex-col border-r border-(--line-1) bg-(--paper-1)">
                {/* §2.5 L874-878 header: Local changes label + Stage all */}
                <div className="flex h-[36px] shrink-0 items-center gap-[8px] border-b border-(--line-1) px-[13px]">
                    <span className="text-[9.5px] font-semibold tracking-[0.07em] text-(--ink-3) uppercase">
                        Local changes
                    </span>
                    <span className="flex-1" />
                    {changesCount > 0 && (
                        <button
                            type="button"
                            onClick={stageAll}
                            className="text-[10.5px] font-semibold text-(--yz-accent-ink)"
                        >
                            Stage all
                        </button>
                    )}
                </div>
                {/* §2.5 L879-889 flat file list — staged first */}
                <div className="yzs min-h-0 flex-1 overflow-y-auto px-[9px] py-[7px]">
                    {rows.map((row) => (
                        <FileRow
                            key={`${row.staged ? "s" : "c"}:${row.path}`}
                            row={row}
                            selected={selectedStaged === row.staged && selectedPath === row.path}
                            onSelect={() => selectGitFile(row.path, row.staged)}
                            onStageToggle={() =>
                                void (row.staged ? unstageOne(row) : stageOne(row))
                            }
                        />
                    ))}
                </div>
            </div>

            {/* right column: diff header (§4.4) + diffMode toggle (§4.2) + body */}
            <div className="flex min-w-0 flex-1 flex-col bg-(--paper-0)">
                <div
                    data-diff-header
                    className="flex h-[38px] shrink-0 items-center gap-[10px] border-b border-(--line-1) bg-(--yz-sunk) px-[16px]"
                >
                    {selectedBadge && <GitBadge badge={selectedBadge} />}
                    <span className="min-w-0 flex-1 truncate font-mono text-[12px] font-semibold text-(--ink-1)">
                        {selectedPath ?? ""}
                    </span>
                    {/* §2.5 L897 language label — mono 11px --ink-3 (omitted when unknown) */}
                    {selectedLang && (
                        <span className="shrink-0 font-mono text-[11px] text-(--ink-3)">
                            {selectedLang}
                        </span>
                    )}
                    {/* §2.5 L898-899 +N / −N line stats — mono 11px 600 mint / danger */}
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
                    {/* §4.2 diffMode toggle: outer --yz-sunk pill, h26 r7 buttons */}
                    <div className="flex shrink-0 gap-[3px] rounded-[9px] bg-(--yz-sunk) p-[3px]">
                        {(["unified", "split"] as const).map((m) => (
                            <button
                                key={m}
                                type="button"
                                aria-pressed={diffMode === m}
                                onClick={() => setDiffMode(m)}
                                className={
                                    "h-[26px] rounded-[7px] px-[12px] text-[11px] font-semibold transition-all duration-[140ms] " +
                                    (diffMode === m
                                        ? "bg-(--yz-solid) text-(--ink-0) shadow-(--shadow-xs)"
                                        : "text-(--ink-3)")
                                }
                            >
                                {m === "unified" ? "Unified" : "Split"}
                            </button>
                        ))}
                    </div>
                </div>
                <div className="min-h-0 flex-1 overflow-hidden">
                    {diff ? (
                        <DiffView content={diff} mode={diffMode} />
                    ) : (
                        <div className="flex h-full items-center justify-center text-[12.5px] text-(--ink-3)">
                            Select a file to view its diff
                        </div>
                    )}
                </div>
            </div>
        </div>
    )
}

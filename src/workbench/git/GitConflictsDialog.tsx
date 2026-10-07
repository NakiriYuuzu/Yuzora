import { useEffect, useMemo, useState } from "react"
import { useTranslation } from "react-i18next"

import { Button } from "@/components/ui/button"
import { Checkbox } from "@/components/ui/checkbox"
import { ScrollArea } from "@/components/ui/scroll-area"
import {
    Dialog,
    DialogContent,
    DialogDescription,
    DialogFooter,
    DialogHeader,
    DialogTitle,
    dialogMinSize
} from "@/components/ui/dialog"
import { logUserAction } from "@/features/logs/userAction"
import { gitConflictResolve } from "@/lib/ipc"
import { useGitConflictStore } from "@/state/gitConflictStore"
import { useGitStore } from "@/state/gitStore"
import { useOverlayPresence } from "@/state/overlayStore"
import { canMergeConflict, conflictSideChanges } from "./conflictSides"


/**
 * JetBrains-style Conflicts dialog: every unmerged file with what each side
 * did, resolved by accepting a whole side or by opening the merge tool.
 */
export function GitConflictsDialog() {
    const { t } = useTranslation("menus")
    const open = useGitConflictStore((s) => s.conflictsOpen)
    const close = useGitConflictStore((s) => s.closeConflicts)
    const openMerge = useGitConflictStore((s) => s.openMerge)
    const status = useGitStore((s) => s.status)
    const busy = useGitStore((s) => s.busy)
    const snapshotStale = useGitStore((s) => s.snapshotStale)
    const runOp = useGitStore((s) => s.runOp)
    const repositoryRoot = useGitStore((s) => s.environment?.status === "ready" ? s.environment.root : null)
    const conflicted = useMemo(() => status?.conflicted ?? [], [status])
    const [checked, setChecked] = useState<string[]>([])
    // Only selections that are still conflicted after each refresh count.
    const selected = checked.filter((path) => conflicted.some((entry) => entry.path === path))
    useOverlayPresence(open)

    useEffect(() => {
        if (open && conflicted.length === 0) close()
    }, [conflicted, open, close])

    const disabled = busy != null || snapshotStale || !repositoryRoot
    const targets = selected.length ? selected : conflicted.length === 1 ? [conflicted[0].path] : []
    const mergeTarget = targets.length === 1 ? conflicted.find((entry) => entry.path === targets[0]) : undefined

    const accept = async (side: "ours" | "theirs") => {
        if (disabled || !repositoryRoot || !targets.length) return
        const paths = [...targets]
        const ok = await runOp(side === "ours" ? "conflict-accept-ours" : "conflict-accept-theirs", () =>
            gitConflictResolve(repositoryRoot, paths, side))
        if (ok) void logUserAction("git_conflict_accept", `accept ${side} (${paths.length})`)
    }

    const toggle = (path: string, value: boolean) =>
        setChecked(value ? [...new Set([...selected, path])] : selected.filter((current) => current !== path))
    const sideLabel = (change: "modified" | "added" | "deleted") => t(`gitConflicts.side.${change}`)

    return (
        <Dialog open={open} onOpenChange={(next) => { if (!next) close() }}>
            <DialogContent
                resizeId="git-conflicts"
                minSize={dialogMinSize(560, 320)}
                className="flex min-h-0 flex-col gap-[12px]"
            >
                <DialogHeader>
                    <DialogTitle>{t("gitConflicts.title")}</DialogTitle>
                    <DialogDescription>
                        {status?.inProgress
                            ? t("gitConflicts.descriptionOp", { op: status.inProgress, count: conflicted.length })
                            : t("gitConflicts.description", { count: conflicted.length })}
                    </DialogDescription>
                </DialogHeader>
                <div className="flex min-h-0 flex-1 gap-[12px]">
                    <ScrollArea className="min-h-0 flex-1 rounded-[8px] border border-(--line-1)">
                        <div role="table" aria-label={t("gitConflicts.title")}>
                            <div role="row" className="sticky top-0 grid grid-cols-[28px_1fr_96px_96px] gap-[8px] border-b border-(--line-1) bg-(--paper-1) px-[8px] py-[6px] text-[11px] font-semibold text-(--ink-3)">
                                <span role="columnheader" />
                                <span role="columnheader">{t("gitConflicts.columnName")}</span>
                                <span role="columnheader">{t("gitConflicts.columnYours")}</span>
                                <span role="columnheader">{t("gitConflicts.columnTheirs")}</span>
                            </div>
                            {conflicted.map((entry) => {
                                const changes = conflictSideChanges(entry.status)
                                const isChecked = selected.includes(entry.path)
                                return (
                                    <div
                                        key={entry.path}
                                        role="row"
                                        aria-selected={isChecked}
                                        className="grid grid-cols-[28px_1fr_96px_96px] items-center gap-[8px] px-[8px] py-[5px] text-[12px] hover:bg-(--yz-hover)"
                                        onDoubleClick={() => { if (!disabled && canMergeConflict(entry.status)) openMerge(entry.path) }}
                                    >
                                        <span role="cell">
                                            <Checkbox
                                                aria-label={t("gitConflicts.selectFile", { path: entry.path })}
                                                checked={isChecked}
                                                onCheckedChange={(value) => toggle(entry.path, value === true)}
                                            />
                                        </span>
                                        <span role="cell" className="truncate font-mono" title={entry.path}>{entry.path}</span>
                                        <span role="cell" className="text-(--ink-2)">{sideLabel(changes.ours)}</span>
                                        <span role="cell" className="text-(--ink-2)">{sideLabel(changes.theirs)}</span>
                                    </div>
                                )
                            })}
                        </div>
                    </ScrollArea>
                    <div className="flex w-[150px] shrink-0 flex-col gap-[6px]">
                        <Button type="button" size="sm" variant="outline" disabled={disabled || !targets.length} onClick={() => void accept("ours")}>
                            {t("gitConflicts.acceptYours")}
                        </Button>
                        <Button type="button" size="sm" variant="outline" disabled={disabled || !targets.length} onClick={() => void accept("theirs")}>
                            {t("gitConflicts.acceptTheirs")}
                        </Button>
                        <Button
                            type="button"
                            size="sm"
                            disabled={disabled || !mergeTarget || !canMergeConflict(mergeTarget.status)}
                            onClick={() => { if (mergeTarget) openMerge(mergeTarget.path) }}
                        >
                            {t("gitConflicts.merge")}
                        </Button>
                        <p className="text-[10.5px] leading-[1.4] text-(--ink-3)">{t("gitConflicts.hint")}</p>
                    </div>
                </div>
                <DialogFooter>
                    <Button type="button" variant="ghost" onClick={close}>{t("gitConflicts.close")}</Button>
                </DialogFooter>
            </DialogContent>
        </Dialog>
    )
}

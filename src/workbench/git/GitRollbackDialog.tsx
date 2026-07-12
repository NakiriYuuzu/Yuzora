import { useEffect, useMemo, useState } from "react"
import { useTranslation } from "react-i18next"

import { Button } from "@/components/ui/button"
import {
    Dialog,
    DialogContent,
    DialogDescription,
    DialogFooter,
    DialogHeader,
    DialogTitle
} from "@/components/ui/dialog"
import { gitRollbackPaths, type GitRollbackResult } from "@/lib/ipc"
import { useGitRollbackDialogStore } from "@/state/gitRollbackDialogStore"
import { useGitStore } from "@/state/gitStore"
import { useUiStore } from "@/state/uiStore"
import { useWorkspaceStore, type EditorGroup } from "@/state/workspaceStore"

import {
    exactGitChanges,
    gitChangeRows,
    rollbackTargetsFromKeys,
    type GitChangeKey
} from "./gitChangeSelection"

function absolutePath(root: string, path: string): string {
    return `${root.replace(/\/$/, "")}/${path}`
}

function tabMatchesRepoPath(tabPath: string, root: string, path: string): boolean {
    const target = absolutePath(root, path).replace(/\/$/, "")
    return tabPath === target || tabPath.startsWith(`${target}/`)
}

function tabsAffectedByPaths(
    root: string,
    paths: readonly string[],
    groups: readonly EditorGroup[]
): string[] {
    return [...new Set(groups.flatMap((group) =>
        group.tabs
            .filter((tab) => paths.some((path) => tabMatchesRepoPath(tab.path, root, path)))
            .map((tab) => tab.path)
    ))]
}

function dirtySelectedPaths(root: string, paths: readonly string[], groups: readonly EditorGroup[]): string[] {
    const affected = new Set(tabsAffectedByPaths(root, paths, groups))
    return [...new Set(groups.flatMap((group) =>
        group.tabs.filter((tab) => tab.dirty && affected.has(tab.path)).map((tab) => tab.path)
    ))]
}

function affectedPaths(targets: readonly GitChangeKey[]): string[] {
    return [...new Set(targets.flatMap((target) =>
        target.origPath ? [target.path, target.origPath] : [target.path]
    ))]
}

function validateTargets(targets: readonly GitChangeKey[]): GitChangeKey[] {
    const status = useGitStore.getState().status
    return exactGitChanges(targets, gitChangeRows(status))
}

function classificationLabel(kind: GitChangeKey["classification"], t: (key: string) => string): string {
    return t(`gitRollbackDialog.classification.${kind}`)
}

export function GitRollbackDialog() {
    const { t } = useTranslation("menus")
    const pending = useGitRollbackDialogStore((state) => state.pending)
    const respond = useGitRollbackDialogStore((state) => state.respond)
    const setError = useGitRollbackDialogStore((state) => state.setError)
    const status = useGitStore((state) => state.status)
    const busy = useGitStore((state) => state.busy)
    const groups = useWorkspaceStore((state) => state.groups)
    const [deleteUntrackedOrAdded, setDeleteUntrackedOrAdded] = useState(false)
    const [submitting, setSubmitting] = useState(false)

    useEffect(() => {
        setDeleteUntrackedOrAdded(false)
        setSubmitting(false)
    }, [pending?.requestId])

    const latest = useMemo(
        () => pending ? exactGitChanges(pending.targets, gitChangeRows(status)) : [],
        [pending, status]
    )
    const stale = Boolean(pending && latest.length !== pending.targets.length)
    const paths = affectedPaths(pending?.targets ?? [])
    const dirty = pending ? dirtySelectedPaths(pending.repositoryRoot, paths, groups) : []
    const conflicted = latest.some((target) => target.classification === "conflicted")
    const hasDeletable = latest.some((target) =>
        target.classification === "untracked" || target.classification === "added"
    )
    const blockedReason = stale
        ? t("gitRollbackDialog.stale")
        : conflicted
            ? t("gitRollbackDialog.conflictBlocked")
            : dirty.length > 0
                ? t("gitRollbackDialog.dirtyBlocked")
                : busy
                    ? t("gitRollbackDialog.busy", { operation: busy })
                    : null

    async function confirmRollback() {
        const request = useGitRollbackDialogStore.getState().pending
        if (!request) return
        const environment = useGitStore.getState().environment
        if (environment?.status !== "ready" || environment.root !== request.repositoryRoot) {
            setError(t("gitRollbackDialog.stale"))
            return
        }

        const current = validateTargets(request.targets)
        if (current.length !== request.targets.length) {
            setError(t("gitRollbackDialog.stale"))
            return
        }
        if (current.some((target) => target.classification === "conflicted")) {
            setError(t("gitRollbackDialog.conflictBlocked"))
            return
        }
        const currentPaths = affectedPaths(current)
        if (dirtySelectedPaths(request.repositoryRoot, currentPaths, useWorkspaceStore.getState().groups).length > 0) {
            setError(t("gitRollbackDialog.dirtyBlocked"))
            return
        }
        if (useGitStore.getState().busy) {
            setError(t("gitRollbackDialog.busy", { operation: useGitStore.getState().busy }))
            return
        }

        let result: GitRollbackResult | null = null
        setError(null)
        setSubmitting(true)
        const ok = await useGitStore.getState().runOp("rollback", async () => {
            result = await gitRollbackPaths(
                request.repositoryRoot,
                rollbackTargetsFromKeys(request.targets),
                deleteUntrackedOrAdded
            )
        })
        if (!ok || !result) {
            const rollbackError = useGitStore.getState().lastError ?? t("gitRollbackDialog.failed")
            // The backend may have completed an earlier rollback step before a
            // later step failed. Refresh and reconcile best-effort, but keep the
            // dialog open and report failure rather than pretending atomic success.
            await useGitStore.getState().refresh()
            useUiStore.getState().reconcileGitChangeSelection(
                gitChangeRows(useGitStore.getState().status)
            )
            setError(rollbackError)
            setSubmitting(false)
            return
        }

        useUiStore.getState().reconcileGitChangeSelection(
            gitChangeRows(useGitStore.getState().status)
        )
        const latestGroups = useWorkspaceStore.getState().groups
        const deletedTabs = tabsAffectedByPaths(
            request.repositoryRoot,
            (result as GitRollbackResult).deleted,
            latestGroups
        )
        const dirtyNow = new Set(latestGroups.flatMap((group) =>
            group.tabs.filter((tab) => tab.dirty).map((tab) => tab.path)
        ))
        useWorkspaceStore.getState().closeTabsByPath(
            deletedTabs.filter((path) => !dirtyNow.has(path))
        )
        respond(true)
    }

    return (
        <Dialog
            open={pending !== null}
            onOpenChange={(open) => {
                if (!open && !submitting) respond(false)
            }}
        >
            {pending && (
                <DialogContent showCloseButton={false} className="sm:max-w-[480px]">
                    <DialogHeader>
                        <DialogTitle>{t("gitRollbackDialog.title")}</DialogTitle>
                        <DialogDescription>{t("gitRollbackDialog.description")}</DialogDescription>
                    </DialogHeader>

                    <ul className="max-h-[220px] space-y-1 overflow-y-auto rounded-lg border border-(--line-1) bg-(--yz-panel) p-2">
                        {rollbackTargetsFromKeys(pending.targets).map((target) => (
                            <li key={target.path} className="flex items-center gap-2 rounded-md px-2 py-1.5">
                                <span className="min-w-0 flex-1 truncate font-mono text-[12px]">{target.path}</span>
                                <span className="shrink-0 text-[10px] text-(--ink-3)">
                                    {classificationLabel(target.classification.kind, t)}
                                </span>
                            </li>
                        ))}
                    </ul>

                    {hasDeletable && (
                        <label className="flex items-start gap-2 text-[12px] text-(--ink-2)">
                            <input
                                type="checkbox"
                                checked={deleteUntrackedOrAdded}
                                disabled={submitting}
                                onChange={(event) => setDeleteUntrackedOrAdded(event.target.checked)}
                            />
                            <span>{t("gitRollbackDialog.deleteUntrackedOrAdded")}</span>
                        </label>
                    )}

                    {(blockedReason || pending.error) && (
                        <p role="alert" className="text-[12px] text-destructive">
                            {pending.error ?? blockedReason}
                        </p>
                    )}

                    <DialogFooter>
                        <Button
                            variant="ghost"
                            disabled={submitting}
                            onClick={() => respond(false)}
                        >
                            {t("gitRollbackDialog.cancel")}
                        </Button>
                        <Button
                            variant="destructive"
                            disabled={Boolean(blockedReason) || busy !== null || submitting}
                            onClick={() => void confirmRollback()}
                        >
                            {submitting ? t("gitRollbackDialog.rollingBack") : t("gitRollbackDialog.confirm")}
                        </Button>
                    </DialogFooter>
                </DialogContent>
            )}
        </Dialog>
    )
}

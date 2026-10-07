import { useTranslation } from "react-i18next"

import { Button } from "@/components/ui/button"
import { gitConflictAbort, gitConflictContinue, gitConflictSkip } from "../../lib/ipc"
import { logUserAction } from "@/features/logs/userAction"
import { useGitStore } from "../../state/gitStore"
import { useUiStore } from "../../state/uiStore"
import { useGitConflictStore } from "@/state/gitConflictStore"
import { requestAppConfirmation } from "@/state/appDialogStore"

/**
 * Merge-conflict banner shown above the Git tabs while an operation is in
 * progress (merge / rebase / cherry-pick / revert). Lists the conflicted files
 * — clicking one selects it in the Local changes view (selectGitFile → the
 * LocalChangesTab effect loads its diff). Abort and Continue both confirm first,
 * then run through useGitStore.runOp; a Continue that fails because the index is
 * still unmerged surfaces via lastError below the banner. No design reference —
 * tokens extended (danger-soft track), px sizing.
 */
export function ConflictBanner() {
    const { t } = useTranslation("menus")
    const status = useGitStore((s) => s.status)
    const runOp = useGitStore((s) => s.runOp)
    const lastError = useGitStore((s) => s.lastError)
    const busy = useGitStore((s) => s.busy)
    const snapshotStale = useGitStore((s) => s.snapshotStale)
    const repositoryRoot = useGitStore((s) => s.environment?.status === "ready" ? s.environment.root : null)
    const selectGitFile = useUiStore((s) => s.selectGitFile)
    const openConflicts = useGitConflictStore((s) => s.openConflicts)

    const op = status?.inProgress ?? null
    const mutationsDisabled = busy != null || snapshotStale || !repositoryRoot
    const conflicted = status?.conflicted ?? []
    // Conflicts can also outlive an operation (e.g. a stash pop).
    if (!op && !conflicted.length) return null

    const resolveButton = conflicted.length > 0 && (
        <Button
            type="button"
            size="xs"
            disabled={mutationsDisabled}
            onClick={openConflicts}
            className="h-auto shrink-0 rounded-[6px] bg-[#c2293f] px-[10px] py-[3px] text-[11px] font-semibold text-(--paper-0) hover:bg-[#c2293f] hover:opacity-90"
        >
            {t("conflictBanner.resolve", { count: conflicted.length })}
        </Button>
    )

    if (!op) {
        return (
            <div className="shrink-0 border-b border-(--line-1)">
                <div className="flex items-center gap-[10px] bg-(--danger-soft) px-[12px] py-[8px]">
                    <span className="flex-1 text-[11.5px] font-semibold text-[#c2293f]">
                        {t("conflictBanner.unmerged", { count: conflicted.length })}
                    </span>
                    {resolveButton}
                </div>
            </div>
        )
    }

    // Arrow bindings (not hoisted function declarations) so TypeScript keeps the
    // `op` non-null narrowing from the guard above inside these handlers.
    const abort = async () => {
        if (mutationsDisabled || !repositoryRoot) return
        const capturedRoot = repositoryRoot
        const capturedOp = op
        const ok = await requestAppConfirmation({
            title: t("conflictBanner.abort"),
            description: t("conflictBanner.abortConfirm", { op: capturedOp }),
            kind: "warning",
            destructive: true
        })
        if (!ok) return
        // Confirmation gap: require the same ready root and in-progress op.
        const live = useGitStore.getState()
        const liveRoot = live.environment?.status === "ready" ? live.environment.root : null
        if (
            liveRoot !== capturedRoot
            || live.snapshotStale
            || live.busy != null
            || live.status?.inProgress !== capturedOp
        ) return
        const done = await runOp(
            "conflict-abort",
            () => gitConflictAbort(capturedRoot, capturedOp),
            { conflictOp: capturedOp }
        )
        if (done) void logUserAction("git_conflict_abort", `abort ${capturedOp}`)
    }

    const conflictContinue = async () => {
        if (mutationsDisabled || !repositoryRoot) return
        const capturedRoot = repositoryRoot
        const capturedOp = op
        const ok = await requestAppConfirmation({
            title: t("conflictBanner.continue"),
            description: t("conflictBanner.continueConfirm", { op: capturedOp }),
            kind: "warning"
        })
        if (!ok) return
        const live = useGitStore.getState()
        const liveRoot = live.environment?.status === "ready" ? live.environment.root : null
        if (
            liveRoot !== capturedRoot
            || live.snapshotStale
            || live.busy != null
            || live.status?.inProgress !== capturedOp
        ) return
        const done = await runOp(
            "conflict-continue",
            () => gitConflictContinue(capturedRoot, capturedOp),
            { conflictOp: capturedOp }
        )
        if (done) void logUserAction("git_conflict_continue", `continue ${capturedOp}`)
    }

    const skip = async () => {
        if (mutationsDisabled || !repositoryRoot || op === "merge") return
        const capturedRoot = repositoryRoot
        const capturedOp = op
        const ok = await requestAppConfirmation({
            title: t("conflictBanner.skip"),
            description: t("conflictBanner.skipConfirm", { op: capturedOp }),
            confirmLabel: t("conflictBanner.skip"),
            kind: "warning",
            destructive: true
        })
        if (!ok) return
        const live = useGitStore.getState()
        const liveRoot = live.environment?.status === "ready" ? live.environment.root : null
        if (
            liveRoot !== capturedRoot
            || live.snapshotStale
            || live.busy != null
            || live.status?.inProgress !== capturedOp
        ) return
        const done = await runOp(
            "conflict-skip",
            () => gitConflictSkip(capturedRoot, capturedOp),
            { conflictOp: capturedOp }
        )
        if (done) void logUserAction("git_conflict_skip", `skip ${capturedOp}`)
    }

    return (
        <div className="shrink-0 border-b border-(--line-1)">
            <div className="flex items-center gap-[10px] bg-(--danger-soft) px-[12px] py-[8px]">
                <span className="text-[11.5px] font-semibold text-[#c2293f]">
                    {t("conflictBanner.opInProgress", { op })}
                </span>
                <div className="flex min-w-0 flex-1 flex-wrap gap-[6px]">
                    {conflicted.map((entry) => (
                        <button
                            key={entry.path}
                            type="button"
                            onClick={() => selectGitFile(entry.path, false)}
                            title={entry.path}
                            className="max-w-full truncate rounded-[6px] bg-(--paper-0) px-[7px] py-[2px] font-mono text-[10.5px] text-(--ink-1) hover:bg-(--yz-hover)"
                        >
                            {entry.path}
                        </button>
                    ))}
                </div>
                {resolveButton}
                {op !== "merge" && (
                    <Button
                        type="button"
                        size="xs"
                        aria-label={t("conflictBanner.skip")}
                        disabled={mutationsDisabled}
                        onClick={skip}
                        className="h-auto shrink-0 rounded-[6px] border border-[#c2293f] bg-transparent px-[10px] py-[3px] text-[11px] font-semibold text-[#c2293f] transition-colors duration-[130ms] hover:bg-[#c2293f] hover:text-(--paper-0)"
                    >
                        {t("conflictBanner.skip")}
                    </Button>
                )}
                <Button
                    type="button"
                    size="xs"
                    aria-label={t("conflictBanner.abort")}
                    disabled={mutationsDisabled}
                    onClick={abort}
                    className="h-auto shrink-0 rounded-[6px] border border-[#c2293f] bg-transparent px-[10px] py-[3px] text-[11px] font-semibold text-[#c2293f] transition-colors duration-[130ms] hover:bg-[#c2293f] hover:text-(--paper-0)"
                >
                    {t("conflictBanner.abort")}
                </Button>
                <Button
                    type="button"
                    size="xs"
                    aria-label={t("conflictBanner.continue")}
                    disabled={mutationsDisabled}
                    onClick={conflictContinue}
                    className="h-auto shrink-0 rounded-[6px] bg-(--ink-1) px-[10px] py-[3px] text-[11px] font-semibold text-(--paper-0) transition-opacity duration-[130ms] hover:bg-(--ink-1) hover:opacity-90"
                >
                    {t("conflictBanner.continue")}
                </Button>
            </div>
            {lastError && (
                <div className="bg-(--danger-soft) px-[12px] pb-[7px] font-mono text-[10.5px] text-[#c2293f]">
                    {lastError}
                </div>
            )}
        </div>
    )
}

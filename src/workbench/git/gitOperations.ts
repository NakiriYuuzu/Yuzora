import { logUserAction } from "@/features/logs/userAction"
import i18n from "@/lib/i18n"
import {
    gitDeleteBranch,
    gitMergeBranch,
    gitPull,
    gitPush,
    gitRebaseOnto,
    gitRenameBranch,
    gitResetBranch,
    gitRevertCommit
} from "@/lib/ipc"
import type { GitOperationOutcome } from "@/lib/types"
import { requestAppConfirmation } from "@/state/appDialogStore"
import { useGitConflictStore } from "@/state/gitConflictStore"
import { useGitLogStore } from "@/state/gitLogStore"
import { useGitStore } from "@/state/gitStore"
import { requestTextInputDialog } from "@/state/textInputDialogStore"

const t = (key: string, options?: Record<string, unknown>) => i18n.t(key, { ns: "menus", ...options })

/** The ready repository root, or null while Git cannot be mutated. */
export function actionableRoot(): string | null {
    const state = useGitStore.getState()
    if (state.busy != null || state.snapshotStale || state.environment?.status !== "ready") return null
    return state.environment.root
}

function currentBranch(): string | null {
    const status = useGitStore.getState().status
    return status && !status.detached ? status.branch : null
}

/** Renaming or deleting a branch leaves HEAD alone, so the log would keep the old ref labels. */
function reloadLogRefs(root: string): void {
    void useGitLogStore.getState().loadFirstPage(root)
}

/** Runs an operation that may stop on conflicts; opens the Conflicts dialog when it does. */
async function runOutcomeOp(name: string, fn: () => Promise<GitOperationOutcome>): Promise<boolean> {
    let outcome: GitOperationOutcome | null = null
    const ok = await useGitStore.getState().runOp(name, async () => {
        outcome = await fn()
    })
    if (ok && (outcome as GitOperationOutcome | null)?.conflicts) useGitConflictStore.getState().openConflicts()
    return ok
}

const BLOCKED_BY_LOCAL_CHANGES = /Your local changes to the following files would be overwritten by checkout/
const BLOCKED_BY_UNTRACKED = /untracked working tree files would be overwritten by checkout/

/**
 * Runs a checkout. When local changes block it, offers JetBrains' Smart
 * Checkout: stash them, switch, and restore them on the new branch. Untracked
 * files the target also tracks come back over its versions.
 */
export async function switchBranch(
    root: string,
    name: "checkout" | "create-branch",
    target: string,
    run: (smart: boolean) => Promise<GitOperationOutcome>
): Promise<boolean> {
    if (await useGitStore.getState().runOp(name, () => run(false))) return true
    const error = useGitStore.getState().lastError ?? ""
    const untracked = BLOCKED_BY_UNTRACKED.test(error)
    if (!untracked && !BLOCKED_BY_LOCAL_CHANGES.test(error)) return false
    const description = t("gitActions.checkoutProblem", { target })
    const smart = await requestAppConfirmation({
        title: t("gitActions.checkoutProblemTitle"),
        description: untracked ? `${description}\n\n${t("gitActions.checkoutProblemUntracked", { target })}` : description,
        confirmLabel: t("gitActions.smartCheckout"),
        cancelLabel: t("gitActions.dontCheckout"),
        kind: "warning"
    })
    if (!smart || actionableRoot() !== root) return false
    return runOutcomeOp(name, () => run(true))
}

export async function mergeIntoCurrent(branch: string): Promise<void> {
    const root = actionableRoot()
    const current = currentBranch()
    if (!root || !current) return
    const ok = await requestAppConfirmation({
        title: t("gitActions.mergeTitle"),
        description: t("gitActions.mergeConfirm", { branch, current }),
        confirmLabel: t("gitActions.merge")
    })
    if (!ok || actionableRoot() !== root) return
    if (await runOutcomeOp("merge-branch", () => gitMergeBranch(root, branch))) {
        void logUserAction("git_merge_branch", `merge ${branch}`)
    }
}

export async function rebaseCurrentOnto(branch: string): Promise<void> {
    const root = actionableRoot()
    const current = currentBranch()
    if (!root || !current) return
    const ok = await requestAppConfirmation({
        title: t("gitActions.rebaseTitle"),
        description: t("gitActions.rebaseConfirm", { branch, current }),
        confirmLabel: t("gitActions.rebase"),
        kind: "warning"
    })
    if (!ok || actionableRoot() !== root) return
    if (await runOutcomeOp("rebase-onto", () => gitRebaseOnto(root, branch))) {
        void logUserAction("git_rebase_onto", `rebase onto ${branch}`)
    }
}

export async function renameBranch(branch: string): Promise<void> {
    const root = actionableRoot()
    if (!root) return
    const next = (await requestTextInputDialog({
        title: t("gitActions.renameTitle", { branch }),
        label: t("gitActions.newName"),
        initialValue: branch,
        confirmLabel: t("gitActions.rename")
    }))?.trim()
    if (!next || next === branch || actionableRoot() !== root) return
    if (await useGitStore.getState().runOp("rename-branch", () => gitRenameBranch(root, branch, next))) {
        reloadLogRefs(root)
        void logUserAction("git_rename_branch", `rename ${branch} → ${next}`)
    }
}

export async function deleteBranch(branch: string): Promise<void> {
    const root = actionableRoot()
    if (!root) return
    const ok = await requestAppConfirmation({
        title: t("gitActions.deleteTitle"),
        description: t("gitActions.deleteConfirm", { branch }),
        confirmLabel: t("gitActions.delete"),
        kind: "warning",
        destructive: true
    })
    if (!ok || actionableRoot() !== root) return
    const store = useGitStore.getState()
    if (await store.runOp("delete-branch", () => gitDeleteBranch(root, branch, false))) {
        reloadLogRefs(root)
        void logUserAction("git_delete_branch", `delete ${branch}`)
        return
    }
    if (!/not fully merged/i.test(useGitStore.getState().lastError ?? "")) return
    const force = await requestAppConfirmation({
        title: t("gitActions.forceDeleteTitle"),
        description: t("gitActions.forceDeleteConfirm", { branch }),
        confirmLabel: t("gitActions.forceDelete"),
        kind: "warning",
        destructive: true
    })
    if (!force || actionableRoot() !== root) return
    if (await store.runOp("delete-branch", () => gitDeleteBranch(root, branch, true))) {
        reloadLogRefs(root)
        void logUserAction("git_delete_branch", `force delete ${branch}`)
    }
}

export async function revertCommit(hash: string, subject: string): Promise<void> {
    const root = actionableRoot()
    if (!root) return
    const ok = await requestAppConfirmation({
        title: t("gitActions.revertTitle"),
        description: t("gitActions.revertConfirm", { hash: hash.slice(0, 7), subject }),
        confirmLabel: t("gitActions.revert")
    })
    if (!ok || actionableRoot() !== root) return
    if (await runOutcomeOp("revert-commit", () => gitRevertCommit(root, hash))) {
        void logUserAction("git_revert_commit", `revert ${hash.slice(0, 7)}`)
    }
}

/** JetBrains "Undo Commit": a soft reset that keeps the commit's changes staged. */
export async function undoLastCommit(subject: string): Promise<void> {
    const root = actionableRoot()
    if (!root) return
    const ok = await requestAppConfirmation({
        title: t("gitActions.undoTitle"),
        description: t("gitActions.undoConfirm", { subject }),
        confirmLabel: t("gitActions.undo"),
        kind: "warning"
    })
    if (!ok || actionableRoot() !== root) return
    if (await useGitStore.getState().runOp("reset-branch", () => gitResetBranch(root, "HEAD~1", "soft"))) {
        void logUserAction("git_undo_commit", "undo last commit")
    }
}

export async function pullWithRebase(): Promise<void> {
    const root = actionableRoot()
    if (!root) return
    await useGitStore.getState().runOp("pull-rebase", () => gitPull(root, "rebase"))
}

export async function forcePushWithLease(): Promise<void> {
    const root = actionableRoot()
    const branch = currentBranch()
    if (!root || !branch) return
    const ok = await requestAppConfirmation({
        title: t("gitActions.forcePushTitle"),
        description: t("gitActions.forcePushConfirm", { branch }),
        confirmLabel: t("gitActions.forcePush"),
        kind: "warning",
        destructive: true
    })
    if (!ok || actionableRoot() !== root) return
    if (await useGitStore.getState().runOp("push-force", () => gitPush(root, { forceWithLease: true }))) {
        void logUserAction("git_push_force", `force push ${branch}`)
    }
}

export async function pushWithTags(): Promise<void> {
    const root = actionableRoot()
    if (!root) return
    await useGitStore.getState().runOp("push", () => gitPush(root, { tags: true }))
}

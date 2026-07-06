import { confirm } from "@tauri-apps/plugin-dialog"

import { gitConflictAbort, gitConflictContinue } from "../../lib/ipc"
import { logUserAction } from "@/features/logs/userAction"
import { useGitStore } from "../../state/gitStore"
import { useUiStore } from "../../state/uiStore"

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
    const status = useGitStore((s) => s.status)
    const runOp = useGitStore((s) => s.runOp)
    const lastError = useGitStore((s) => s.lastError)
    const selectGitFile = useUiStore((s) => s.selectGitFile)

    const op = status?.inProgress ?? null
    if (!op) return null

    const conflicted = status?.conflicted ?? []

    // Arrow bindings (not hoisted function declarations) so TypeScript keeps the
    // `op` non-null narrowing from the guard above inside these handlers.
    const abort = async () => {
        const ok = await confirm(`Abort the in-progress ${op}? All ${op} changes will be discarded.`)
        if (!ok) return
        const done = await runOp("conflict-abort", () => gitConflictAbort(op), { conflictOp: op })
        if (done) void logUserAction("git_conflict_abort", `abort ${op}`)
    }

    const conflictContinue = async () => {
        const ok = await confirm(`Continue the ${op}? Resolve all conflicts first.`)
        if (!ok) return
        const done = await runOp("conflict-continue", () => gitConflictContinue(op), { conflictOp: op })
        if (done) void logUserAction("git_conflict_continue", `continue ${op}`)
    }

    return (
        <div className="shrink-0 border-b border-(--line-1)">
            <div className="flex items-center gap-[10px] bg-(--danger-soft) px-[12px] py-[8px]">
                <span className="text-[11.5px] font-semibold text-[#c2293f]">{op} 進行中</span>
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
                <button
                    type="button"
                    aria-label="Abort"
                    onClick={abort}
                    className="shrink-0 rounded-[6px] border border-[#c2293f] px-[10px] py-[3px] text-[11px] font-semibold text-[#c2293f] transition-colors duration-[130ms] hover:bg-[#c2293f] hover:text-(--paper-0)"
                >
                    Abort
                </button>
                <button
                    type="button"
                    aria-label="Continue"
                    onClick={conflictContinue}
                    className="shrink-0 rounded-[6px] bg-(--ink-1) px-[10px] py-[3px] text-[11px] font-semibold text-(--paper-0) transition-opacity duration-[130ms] hover:opacity-90"
                >
                    Continue
                </button>
            </div>
            {lastError && (
                <div className="bg-(--danger-soft) px-[12px] pb-[7px] font-mono text-[10.5px] text-[#c2293f]">
                    {lastError}
                </div>
            )}
        </div>
    )
}

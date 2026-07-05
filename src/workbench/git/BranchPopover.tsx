import { type ReactNode, useEffect, useState } from "react"
import { Check, GitBranch, Globe, Plus, RefreshCw } from "lucide-react"

import {
    Popover,
    PopoverContent,
    PopoverTrigger
} from "@/components/ui/popover"
import { useGitStore } from "@/state/gitStore"
import { useWorkspaceStore } from "@/state/workspaceStore"
import type { BranchInfo } from "@/lib/types"
import { gitCheckout, gitCreateBranch, gitFetch, gitPull, gitPush } from "@/lib/ipc"
import { strings } from "@/lib/i18n"

interface BranchPopoverProps {
    open: boolean
    onOpenChange: (open: boolean) => void
    trigger?: ReactNode
}

function hasDirtyTab(): boolean {
    return useWorkspaceStore
        .getState()
        .groups.some((g) => g.tabs.some((t) => t.dirty))
}

// Design reference §3.3 — Local branch row (h32, pad 0 11px, r9, mono 12px).
// current: bg --yz-active + bold; ✓ #1f8a5b (--term-ok). ahead #2456cc
// (--status-m), behind #c8521f (§0.2 inline hardcode). Non-current rows expose a
// Checkout button on hover; checkout is blocked when unsaved tabs exist (§brief).
function LocalRow({
    branch,
    onBlocked,
    onDone
}: {
    branch: BranchInfo
    onBlocked: () => void
    onDone: () => void
}) {
    const runOp = useGitStore((s) => s.runOp)

    async function checkout() {
        if (hasDirtyTab()) {
            onBlocked()
            return
        }
        const ok = await runOp("checkout", () => gitCheckout(branch.name))
        if (ok) onDone()
    }

    return (
        <div
            className="group flex h-[32px] items-center gap-[9px] rounded-[9px] px-[11px] transition-colors duration-100 hover:bg-(--yz-hover)"
            style={branch.isCurrent ? { background: "var(--yz-active)" } : undefined}
        >
            <Check
                className="size-[14px] shrink-0"
                style={{ color: "var(--term-ok)", opacity: branch.isCurrent ? 1 : 0 }}
                aria-hidden="true"
            />
            <span
                className="font-mono text-[12px] text-(--ink-1)"
                style={{ fontWeight: branch.isCurrent ? 600 : 500 }}
            >
                {branch.name}
            </span>
            {branch.ahead > 0 && (
                <span className="font-mono text-[10px]" style={{ color: "var(--status-m)" }}>
                    ↑{branch.ahead}
                </span>
            )}
            {branch.behind > 0 && (
                <span className="font-mono text-[10px]" style={{ color: "#c8521f" }}>
                    ↓{branch.behind}
                </span>
            )}
            <div className="flex-1" />
            {branch.isCurrent ? (
                <span className="font-mono text-[10px] text-(--ink-3)">current</span>
            ) : (
                <button
                    type="button"
                    onClick={checkout}
                    className="rounded-[6px] px-[8px] py-[2px] text-[10.5px] font-semibold text-(--yz-accent-ink) opacity-0 transition-opacity duration-[130ms] hover:bg-(--yz-hover) group-hover:opacity-100"
                >
                    Checkout
                </button>
            )}
        </div>
    )
}

export function BranchPopover({ open, onOpenChange, trigger }: BranchPopoverProps) {
    const branches = useGitStore((s) => s.branches)
    const busy = useGitStore((s) => s.busy)
    const remotePaused = useGitStore((s) => s.remotePaused)
    const runOp = useGitStore((s) => s.runOp)

    const [notice, setNotice] = useState<string | null>(null)
    const [creating, setCreating] = useState(false)
    const [newName, setNewName] = useState("")

    // Clear the transient notice whenever the popover closes so a stale message
    // (e.g. checkout blocked by unsaved changes) doesn't reappear on next open (T14).
    useEffect(() => {
        if (!open) setNotice(null)
    }, [open])

    const local = branches?.local ?? []
    const remote = branches?.remote ?? []

    async function createBranch() {
        const name = newName.trim()
        if (!name) return
        const ok = await runOp("create-branch", () => gitCreateBranch(name))
        if (ok) {
            setNewName("")
            setCreating(false)
        }
    }

    return (
        <Popover open={open} onOpenChange={onOpenChange}>
            {trigger ? <PopoverTrigger asChild>{trigger}</PopoverTrigger> : <PopoverTrigger />}
            <PopoverContent
                side="top"
                align="start"
                sideOffset={6}
                // §3.1 container — w288, r14, --frost-light, --line-2 border, --shadow-xl.
                className="yz-pop flex max-h-[62vh] w-[288px] flex-col gap-0 rounded-[14px] border border-(--line-2) bg-(--frost-light) p-0 shadow-[var(--shadow-xl)] ring-0 backdrop-blur-[20px]"
            >
                {/* §3.2 header — git svg 15, serif 14px 600 title, Fetch btn h25 r8 */}
                <div className="flex items-center gap-[8px] border-b border-(--line-1) px-[13px] py-[11px]">
                    <GitBranch
                        className="size-[15px] shrink-0"
                        style={{ color: "var(--yz-accent-ink)" }}
                        aria-hidden="true"
                    />
                    <span className="flex-1 font-serif text-[14px] font-semibold text-(--ink-0)">
                        Git Branches
                    </span>
                    <button
                        type="button"
                        aria-label="Fetch remote"
                        disabled={busy != null}
                        onClick={() => runOp("fetch", () => gitFetch(false))}
                        className="flex h-[25px] items-center gap-[5px] rounded-[8px] border border-(--line-1) bg-(--yz-solid) px-[10px] text-[11px] font-semibold text-(--ink-1) shadow-[var(--shadow-xs)] transition-colors hover:bg-(--paper-1) disabled:opacity-50"
                    >
                        <RefreshCw className="size-[12px]" aria-hidden="true" />
                        {busy === "fetch" ? "Fetching…" : "Fetch"}
                    </button>
                </div>

                {/* §3.3 scroll region — pad 7px */}
                <div className="min-h-0 flex-1 overflow-y-auto p-[7px]">
                    {remotePaused && (
                        <div
                            className="mb-[4px] rounded-[9px] border border-(--line-1) px-[11px] py-[7px] text-[11px]"
                            style={{ background: "var(--danger-soft)", color: "var(--status-d)" }}
                        >
                            遠端檢查已暫停：需要認證
                        </div>
                    )}
                    {notice && (
                        <div
                            className="mb-[4px] rounded-[9px] border border-(--line-1) px-[11px] py-[7px] text-[11px]"
                            style={{ background: "var(--danger-soft)", color: "var(--status-d)" }}
                        >
                            {notice}
                        </div>
                    )}

                    {/* Local section — label 9px uppercase 0.1em, pad 5 8 4 */}
                    <div className="px-[8px] pb-[4px] pt-[5px] text-[9px] font-medium uppercase tracking-[0.1em] text-(--ink-3)">
                        Local
                    </div>
                    {local.map((b) => (
                        <LocalRow
                            key={b.name}
                            branch={b}
                            onBlocked={() => setNotice("有未儲存的變更，請先存檔或放棄")}
                            onDone={() => onOpenChange(false)}
                        />
                    ))}

                    {/* Remote section — label pad 9 8 4 (top 9) */}
                    <div className="px-[8px] pb-[4px] pt-[9px] text-[9px] font-medium uppercase tracking-[0.1em] text-(--ink-3)">
                        Remote
                    </div>
                    {remote.map((name) => (
                        <div
                            key={name}
                            className="flex h-[30px] items-center gap-[9px] rounded-[9px] px-[11px] transition-colors duration-100 hover:bg-(--yz-hover)"
                        >
                            <Globe
                                className="size-[12px] shrink-0"
                                style={{ color: "var(--ink-4)" }}
                                aria-hidden="true"
                            />
                            <span className="font-mono text-[11.5px] text-(--ink-3)">{name}</span>
                        </div>
                    ))}
                </div>

                {/* §3.4 New branch… — pad 7px, border-top, row h30 r9 --ink-2 12px 500 */}
                <div className="border-t border-(--line-1) p-[7px]">
                    {creating ? (
                        <input
                            autoFocus
                            value={newName}
                            placeholder={strings.git.branchNamePlaceholder}
                            onChange={(e) => setNewName(e.target.value)}
                            onKeyDown={(e) => {
                                if (e.key === "Enter") createBranch()
                                if (e.key === "Escape") {
                                    setNewName("")
                                    setCreating(false)
                                }
                            }}
                            onBlur={() => {
                                if (!newName.trim()) setCreating(false)
                            }}
                            className="h-[30px] w-full rounded-[9px] border border-(--line-1) bg-(--yz-solid) px-[11px] font-mono text-[12px] text-(--ink-1) outline-none"
                        />
                    ) : (
                        <button
                            type="button"
                            onClick={() => setCreating(true)}
                            className="flex h-[30px] w-full items-center gap-[9px] rounded-[9px] px-[11px] text-[12px] font-medium text-(--ink-2) transition-colors duration-100 hover:bg-(--yz-hover)"
                        >
                            <Plus className="size-[13px] shrink-0" aria-hidden="true" />
                            New branch…
                        </button>
                    )}
                </div>

                {/* Bottom action row — Fetch / Pull / Push through runOp, disabled while busy */}
                <div className="flex items-center gap-[6px] border-t border-(--line-1) px-[7px] py-[7px]">
                    <ActionButton label="Fetch" busy={busy} onClick={() => runOp("fetch", () => gitFetch(false))} />
                    <ActionButton label="Pull" busy={busy} onClick={() => runOp("pull", () => gitPull())} />
                    <ActionButton label="Push" busy={busy} onClick={() => runOp("push", () => gitPush())} />
                </div>
            </PopoverContent>
        </Popover>
    )
}

function ActionButton({
    label,
    busy,
    onClick
}: {
    label: string
    busy: string | null
    onClick: () => void
}) {
    const active = busy === label.toLowerCase()
    return (
        <button
            type="button"
            disabled={busy != null}
            onClick={onClick}
            className="flex h-[30px] flex-1 items-center justify-center gap-[6px] rounded-[9px] border border-(--line-1) bg-(--yz-solid) text-[11.5px] font-semibold text-(--ink-1) shadow-[var(--shadow-xs)] transition-colors hover:bg-(--paper-1) disabled:opacity-50"
        >
            {active ? `${label}…` : label}
        </button>
    )
}

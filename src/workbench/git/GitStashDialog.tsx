import { useCallback, useEffect, useState } from "react"
import { useTranslation } from "react-i18next"

import { Button } from "@/components/ui/button"
import { Checkbox } from "@/components/ui/checkbox"
import {
    Dialog,
    DialogContent,
    DialogDescription,
    DialogFooter,
    DialogHeader,
    DialogTitle,
    dialogMinSize
} from "@/components/ui/dialog"
import { Input } from "@/components/ui/input"
import { Label } from "@/components/ui/label"
import { ScrollArea } from "@/components/ui/scroll-area"
import { logUserAction } from "@/features/logs/userAction"
import { gitStashApply, gitStashDrop, gitStashList, gitStashPush } from "@/lib/ipc"
import type { GitStashEntry } from "@/lib/types"
import { requestAppConfirmation } from "@/state/appDialogStore"
import { useGitActionDialogStore } from "@/state/gitActionDialogStore"
import { useGitConflictStore } from "@/state/gitConflictStore"
import { useGitStore } from "@/state/gitStore"
import { useOverlayPresence } from "@/state/overlayStore"
import { actionableRoot } from "./gitOperations"

/** Stash local changes and apply, pop or drop existing stashes. */
export function GitStashDialog() {
    const { t } = useTranslation("menus")
    const open = useGitActionDialogStore((s) => s.stashOpen)
    const close = useGitActionDialogStore((s) => s.closeStash)
    const busy = useGitStore((s) => s.busy)
    const statusRevision = useGitStore((s) => s.statusRevision)
    const root = useGitStore((s) => s.environment?.status === "ready" ? s.environment.root : null)
    const [message, setMessage] = useState("")
    const [includeUntracked, setIncludeUntracked] = useState(true)
    const [keepIndex, setKeepIndex] = useState(false)
    const [stashes, setStashes] = useState<{ root: string; entries: GitStashEntry[]; error?: string } | null>(null)
    useOverlayPresence(open)

    const load = useCallback(() => {
        if (!root) return
        gitStashList(root)
            .then((entries) => setStashes({ root, entries }))
            .catch((error: unknown) => setStashes({ root, entries: [], error: String(error) }))
    }, [root])

    useEffect(() => {
        // Reload after every accepted status snapshot (stash/pop/drop change it).
        if (open) load()
    }, [open, load, statusRevision])

    const entries = stashes?.root === root ? stashes.entries : []
    const disabled = busy != null || !root

    const stash = async () => {
        const target = actionableRoot()
        if (!target) return
        const ok = await useGitStore.getState().runOp("stash-push", () =>
            gitStashPush(target, message.trim() || null, includeUntracked, keepIndex))
        if (ok) {
            setMessage("")
            void logUserAction("git_stash_push", "stash local changes")
            load()
        }
    }

    const apply = async (entry: GitStashEntry, pop: boolean) => {
        const target = actionableRoot()
        if (!target) return
        let conflicts = false
        const ok = await useGitStore.getState().runOp(pop ? "stash-pop" : "stash-apply", async () => {
            conflicts = (await gitStashApply(target, entry.index, entry.oid, pop)).conflicts
        })
        if (!ok) {
            load() // a stale index aborts in the host; show the renumbered list
            return
        }
        void logUserAction(pop ? "git_stash_pop" : "git_stash_apply", `stash@{${entry.index}}`)
        load()
        if (conflicts) {
            close()
            useGitConflictStore.getState().openConflicts()
        }
    }

    const drop = async (entry: GitStashEntry) => {
        const target = actionableRoot()
        if (!target) return
        const ok = await requestAppConfirmation({
            title: t("gitStash.dropTitle"),
            description: t("gitStash.dropConfirm", { message: entry.message }),
            confirmLabel: t("gitStash.drop"),
            kind: "warning",
            destructive: true
        })
        if (!ok || actionableRoot() !== target) return
        await useGitStore.getState().runOp("stash-drop", () => gitStashDrop(target, entry.index, entry.oid))
        load()
    }

    return (
        <Dialog open={open} onOpenChange={(next) => { if (!next) close() }}>
            <DialogContent resizeId="git-stash" minSize={dialogMinSize(480, 360)} className="flex min-h-0 flex-col gap-[14px]">
                <DialogHeader>
                    <DialogTitle>{t("gitStash.title")}</DialogTitle>
                    <DialogDescription>{t("gitStash.description")}</DialogDescription>
                </DialogHeader>
                <section className="flex flex-col gap-[8px]">
                    <Label htmlFor="git-stash-message">{t("gitStash.message")}</Label>
                    <Input
                        id="git-stash-message"
                        value={message}
                        placeholder={t("gitStash.messagePlaceholder")}
                        onChange={(event) => setMessage(event.target.value)}
                    />
                    <div className="flex flex-wrap items-center gap-[14px] text-[12px]">
                        <Label className="flex items-center gap-[6px] font-normal">
                            <Checkbox checked={includeUntracked} onCheckedChange={(value) => setIncludeUntracked(value === true)} />
                            {t("gitStash.includeUntracked")}
                        </Label>
                        <Label className="flex items-center gap-[6px] font-normal">
                            <Checkbox checked={keepIndex} onCheckedChange={(value) => setKeepIndex(value === true)} />
                            {t("gitStash.keepIndex")}
                        </Label>
                        <span className="flex-1" />
                        <Button type="button" size="sm" disabled={disabled} onClick={() => void stash()}>{t("gitStash.stash")}</Button>
                    </div>
                </section>
                <section aria-label={t("gitStash.listLabel")} className="flex min-h-0 flex-1 flex-col gap-[6px]">
                    <h3 className="text-[12px] font-semibold text-(--ink-2)">{t("gitStash.listLabel")}</h3>
                    {stashes?.error && <p role="alert" className="text-[11px] text-(--danger)">{stashes.error}</p>}
                    {!entries.length ? (
                        <p className="text-[12px] text-(--ink-3)">{t("gitStash.empty")}</p>
                    ) : (
                        <ScrollArea className="min-h-0 flex-1 rounded-[8px] border border-(--line-1)">
                            <ul>
                                {entries.map((entry) => (
                                    <li key={entry.index} className="flex items-center gap-[8px] border-b border-(--line-1) px-[10px] py-[6px] last:border-b-0">
                                        <span className="font-mono text-[11px] text-(--ink-3)">{`stash@{${entry.index}}`}</span>
                                        <span className="min-w-0 flex-1 truncate text-[12px]" title={entry.message}>{entry.message}</span>
                                        <Button type="button" size="xs" variant="outline" disabled={disabled} aria-label={t("gitStash.applyAria", { ref: `stash@{${entry.index}}` })} onClick={() => void apply(entry, false)}>{t("gitStash.apply")}</Button>
                                        <Button type="button" size="xs" variant="outline" disabled={disabled} aria-label={t("gitStash.popAria", { ref: `stash@{${entry.index}}` })} onClick={() => void apply(entry, true)}>{t("gitStash.pop")}</Button>
                                        <Button type="button" size="xs" variant="ghost" disabled={disabled} aria-label={t("gitStash.dropAria", { ref: `stash@{${entry.index}}` })} onClick={() => void drop(entry)}>{t("gitStash.drop")}</Button>
                                    </li>
                                ))}
                            </ul>
                        </ScrollArea>
                    )}
                </section>
                <DialogFooter>
                    <Button type="button" variant="ghost" onClick={close}>{t("gitStash.close")}</Button>
                </DialogFooter>
            </DialogContent>
        </Dialog>
    )
}

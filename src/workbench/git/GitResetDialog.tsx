import { useState } from "react"
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
import { Label } from "@/components/ui/label"
import { RadioGroup, RadioGroupItem } from "@/components/ui/radio-group"
import { logUserAction } from "@/features/logs/userAction"
import { gitResetBranch } from "@/lib/ipc"
import type { GitResetMode } from "@/lib/types"
import { useGitActionDialogStore } from "@/state/gitActionDialogStore"
import { useGitStore } from "@/state/gitStore"
import { useOverlayPresence } from "@/state/overlayStore"
import { actionableRoot } from "./gitOperations"

const MODES: GitResetMode[] = ["soft", "mixed", "hard", "keep"]

/** JetBrains "Reset Current Branch to Here": pick soft / mixed / hard / keep. */
export function GitResetDialog() {
    const { t } = useTranslation("menus")
    const target = useGitActionDialogStore((s) => s.resetTarget)
    const close = useGitActionDialogStore((s) => s.closeReset)
    const branch = useGitStore((s) => s.status?.detached ? null : s.status?.branch ?? null)
    const busy = useGitStore((s) => s.busy)
    const [mode, setMode] = useState<GitResetMode>("mixed")
    useOverlayPresence(target !== null)

    const reset = async () => {
        const root = actionableRoot()
        if (!target || !root) return
        const hash = target.hash
        close()
        const ok = await useGitStore.getState().runOp("reset-branch", () => gitResetBranch(root, hash, mode))
        if (ok) void logUserAction("git_reset_branch", `reset --${mode} ${hash.slice(0, 7)}`)
    }

    return (
        <Dialog open={target !== null} onOpenChange={(next) => { if (!next) close() }}>
            <DialogContent className="flex flex-col gap-[12px] sm:max-w-[480px]">
                <DialogHeader>
                    <DialogTitle>{t("gitReset.title", { branch: branch ?? "HEAD" })}</DialogTitle>
                    <DialogDescription>
                        {target ? t("gitReset.description", { hash: target.hash.slice(0, 7), subject: target.subject }) : null}
                    </DialogDescription>
                </DialogHeader>
                <RadioGroup value={mode} onValueChange={(value) => setMode(value as GitResetMode)} className="gap-[10px]">
                    {MODES.map((value) => (
                        <div key={value} className="flex items-start gap-[8px]">
                            <RadioGroupItem id={`git-reset-${value}`} value={value} className="mt-[2px]" />
                            <Label htmlFor={`git-reset-${value}`} className="flex flex-col items-start gap-[2px] font-normal">
                                <span className="font-semibold">{t(`gitReset.mode.${value}`)}</span>
                                <span className="text-[11px] text-(--ink-3)">{t(`gitReset.modeHint.${value}`)}</span>
                            </Label>
                        </div>
                    ))}
                </RadioGroup>
                <DialogFooter>
                    <Button type="button" variant="ghost" onClick={close}>{t("gitReset.cancel")}</Button>
                    <Button
                        type="button"
                        variant={mode === "hard" ? "destructive" : "default"}
                        disabled={busy != null}
                        onClick={() => void reset()}
                    >
                        {t("gitReset.reset")}
                    </Button>
                </DialogFooter>
            </DialogContent>
        </Dialog>
    )
}

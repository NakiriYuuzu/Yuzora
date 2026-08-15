import { useEffect } from "react"
import { useTranslation } from "react-i18next"

import {
    AlertDialog,
    AlertDialogAction,
    AlertDialogCancel,
    AlertDialogContent,
    AlertDialogDescription,
    AlertDialogFooter,
    AlertDialogHeader,
    AlertDialogTitle
} from "@/components/ui/alert-dialog"
import { Button } from "@/components/ui/button"
import { ScrollArea } from "@/components/ui/scroll-area"
import { useGitStore } from "@/state/gitStore"
import { useOverlayPresence } from "@/state/overlayStore"
import { useWorkspaceStore } from "@/state/workspaceStore"
import { useWorkspaceTrustStore } from "@/state/workspaceTrustStore"

export function WorkspaceTrustHost() {
    const { t } = useTranslation("workbench")
    const workspacePath = useWorkspaceStore((state) => state.workspacePath)
    const prompt = useWorkspaceTrustStore((state) => state.prompt)
    const lastError = useWorkspaceTrustStore((state) => state.lastError)
    const trustRevision = useWorkspaceTrustStore((state) => state.trustRevision)
    const confirmPrompt = useWorkspaceTrustStore((state) => state.confirmPrompt)
    const cancelPrompt = useWorkspaceTrustStore((state) => state.cancelPrompt)
    useOverlayPresence(prompt !== null)

    useEffect(() => {
        useWorkspaceTrustStore.getState().cancelPrompt()
        if (!workspacePath) return
        let cancelled = false
        void (async () => {
            try {
                const status = await useWorkspaceTrustStore.getState().refreshStatus(workspacePath)
                if (cancelled) return
                if (status.state === "trusted") return
                if (!status.challengeId) return
                if (status.state !== "invalid" && status.repoPresent !== true) return
                const granted = await useWorkspaceTrustStore.getState().requestWorkspaceGrant(status)
                if (cancelled || !granted) return
                if (useWorkspaceStore.getState().workspacePath !== workspacePath) return
                await useGitStore.getState().detect(workspacePath)
            } catch {
                // Status / grant errors stay in the trust store.
            }
        })()
        return () => {
            cancelled = true
        }
    }, [workspacePath, trustRevision])

    const open = prompt !== null
    const execute = prompt?.kind === "execute" ? prompt : null

    return (
        <AlertDialog
            open={open}
            onOpenChange={(next) => {
                if (!next) cancelPrompt()
            }}
        >
            <AlertDialogContent>
                <AlertDialogHeader>
                    <AlertDialogTitle>
                        {execute ? t("workspaceTrust.executeTitle") : t("workspaceTrust.title")}
                    </AlertDialogTitle>
                    <AlertDialogDescription>
                        {execute
                            ? t("workspaceTrust.executeDescription")
                            : t("workspaceTrust.description")}
                    </AlertDialogDescription>
                </AlertDialogHeader>
                {prompt ? (
                    <div className="grid gap-[10px] text-[12.5px]">
                        <div>
                            <p className="mb-[4px] text-[11px] font-medium uppercase tracking-[0.04em] text-(--ink-3)">
                                {t("workspaceTrust.workspaceLabel")}
                            </p>
                            <p className="break-all font-mono text-(--ink-1)">{prompt.canonicalPath}</p>
                        </div>
                        {execute ? (
                            <div>
                                <div className="mb-[4px] flex items-center justify-between gap-[8px]">
                                    <p className="text-[11px] font-medium uppercase tracking-[0.04em] text-(--ink-3)">
                                        {t("workspaceTrust.commandLabel")}
                                    </p>
                                    <Button
                                        type="button"
                                        variant="ghost"
                                        size="sm"
                                        className="h-[22px] px-[8px] text-[11px]"
                                        aria-label={t("workspaceTrust.copyCommand")}
                                        onClick={() => void navigator.clipboard.writeText(execute.command)}
                                    >
                                        {t("workspaceTrust.copyCommand")}
                                    </Button>
                                </div>
                                <ScrollArea className="max-h-[140px] rounded-[8px] border border-(--line-1) bg-(--yz-sunk)">
                                    <pre
                                        data-testid="workspace-trust-command"
                                        className="p-[8px] font-mono text-[12px] text-(--ink-1) whitespace-pre-wrap break-all"
                                    >
                                        {execute.command}
                                    </pre>
                                </ScrollArea>
                                <p className="mt-[8px] text-[12px] text-(--ink-3)">
                                    {t("workspaceTrust.connectExistingHint")}
                                </p>
                            </div>
                        ) : null}
                        {lastError ? (
                            <p className="text-[12px] text-[#b4232a]">{lastError}</p>
                        ) : null}
                    </div>
                ) : null}
                <AlertDialogFooter>
                    <AlertDialogCancel>{t("workspaceTrust.cancel")}</AlertDialogCancel>
                    <AlertDialogAction onClick={() => void confirmPrompt()}>
                        {execute ? t("workspaceTrust.runCommand") : t("workspaceTrust.grant")}
                    </AlertDialogAction>
                </AlertDialogFooter>
            </AlertDialogContent>
        </AlertDialog>
    )
}

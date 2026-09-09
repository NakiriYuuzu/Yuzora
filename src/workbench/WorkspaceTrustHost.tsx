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
import { workspacePathForDisplay } from "@/lib/paths"
import { useGitStore } from "@/state/gitStore"
import { useOverlayPresence } from "@/state/overlayStore"
import { useWorkspaceStore } from "@/state/workspaceStore"
import { useWorkspaceTrustStore } from "@/state/workspaceTrustStore"

export function WorkspaceTrustHost() {
    const { t } = useTranslation("workbench")
    const workspacePath = useWorkspaceStore((state) => state.workspacePath)
    const prompt = useWorkspaceTrustStore((state) => state.prompt)
    const lastError = useWorkspaceTrustStore((state) => state.lastError)
    const confirming = useWorkspaceTrustStore((state) => state.confirming)
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
                        {t("workspaceTrust.title")}
                    </AlertDialogTitle>
                    <AlertDialogDescription>
                        {t("workspaceTrust.description")}
                    </AlertDialogDescription>
                </AlertDialogHeader>
                {prompt ? (
                    <div className="grid gap-[10px] text-[12.5px]">
                        <div>
                            <p className="mb-[4px] text-[11px] font-medium uppercase tracking-[0.04em] text-(--ink-3)">
                                {t("workspaceTrust.workspaceLabel")}
                            </p>
                            <p className="break-all font-mono text-(--ink-1)">
                                {workspacePathForDisplay(prompt.canonicalPath)}
                            </p>
                        </div>
                        {lastError ? (
                            <p className="text-[12px] text-[#b4232a]">{lastError}</p>
                        ) : null}
                    </div>
                ) : null}
                <AlertDialogFooter>
                    <AlertDialogCancel>{t("workspaceTrust.cancel")}</AlertDialogCancel>
                    <AlertDialogAction
                        disabled={confirming}
                        onClick={(event) => {
                            event.preventDefault()
                            void confirmPrompt()
                        }}
                    >
                        {t("workspaceTrust.grant")}
                    </AlertDialogAction>
                </AlertDialogFooter>
            </AlertDialogContent>
        </AlertDialog>
    )
}

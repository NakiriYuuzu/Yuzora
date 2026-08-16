import { useEffect, useRef, useState } from "react"
import { listen } from "@tauri-apps/api/event"
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
import { sshHostKeyRespond } from "@/lib/ipc"
import type { SshHostKeyPrompt } from "@/lib/types"
import { useOverlayPresence } from "@/state/overlayStore"

export function SshHostKeyHost() {
    const { t } = useTranslation("workbench")
    const [queue, setQueue] = useState<SshHostKeyPrompt[]>([])
    const [error, setError] = useState<string | null>(null)

    useEffect(() => {
        const unlisten = listen<SshHostKeyPrompt>("ssh://host-key-prompt", (event) => {
            setQueue((current) => [...current, event.payload])
        })
        return () => {
            void unlisten.then((fn) => fn())
        }
    }, [])

    const current = queue[0] ?? null
    const settledRef = useRef(false)
    useOverlayPresence(current !== null)

    function dequeue() {
        setQueue((items) => items.slice(1))
        setError(null)
        settledRef.current = false
    }

    async function rejectCurrent() {
        if (!current || settledRef.current) return
        settledRef.current = true
        if (current.kind === "new") {
            try {
                await sshHostKeyRespond(current.challengeId, false, current.endpoint, current.fingerprint)
            } catch {
                // Handshake already failed closed; drop the prompt either way.
            }
        }
        dequeue()
    }

    async function acceptCurrent() {
        if (!current || current.kind !== "new" || settledRef.current) return
        settledRef.current = true
        try {
            await sshHostKeyRespond(current.challengeId, true, current.endpoint, current.fingerprint)
            dequeue()
        } catch (cause) {
            settledRef.current = false
            setError(String(cause))
        }
    }

    function copyFingerprint() {
        if (!current) return
        void navigator.clipboard.writeText(current.fingerprint)
    }

    if (!current) return null

    const isNew = current.kind === "new"

    return (
        <AlertDialog
            open
            onOpenChange={(open) => {
                if (!open) void rejectCurrent()
            }}
        >
            <AlertDialogContent data-testid="ssh-host-key-dialog">
                <AlertDialogHeader>
                    <AlertDialogTitle>
                        {isNew ? t("sshHostKey.newTitle") : t("sshHostKey.changedTitle")}
                    </AlertDialogTitle>
                    <AlertDialogDescription>
                        {isNew ? t("sshHostKey.newDescription") : t("sshHostKey.changedDescription")}
                    </AlertDialogDescription>
                </AlertDialogHeader>
                <div className="grid gap-[10px] text-[12.5px]">
                    <div>
                        <p className="mb-[4px] text-[11px] font-medium uppercase tracking-[0.04em] text-(--ink-3)">
                            {t("sshHostKey.hostLabel")}
                        </p>
                        <p className="break-all font-mono text-(--ink-1)">{current.endpoint}</p>
                    </div>
                    <div>
                        <p className="mb-[4px] text-[11px] font-medium uppercase tracking-[0.04em] text-(--ink-3)">
                            {t("sshHostKey.algorithmLabel")}
                        </p>
                        <p className="font-mono text-(--ink-1)">{current.algorithm}</p>
                    </div>
                    {current.kind === "changed" ? (
                        <>
                            <div>
                                <p className="mb-[4px] text-[11px] font-medium uppercase tracking-[0.04em] text-(--ink-3)">
                                    {t("sshHostKey.previousFingerprintLabel")}
                                </p>
                                <p
                                    data-testid="ssh-host-key-previous"
                                    className="break-all font-mono text-(--ink-1)"
                                >
                                    {current.previousFingerprint}
                                </p>
                            </div>
                            <div>
                                <div className="mb-[4px] flex items-center justify-between gap-[8px]">
                                    <p className="text-[11px] font-medium uppercase tracking-[0.04em] text-(--ink-3)">
                                        {t("sshHostKey.presentedFingerprintLabel")}
                                    </p>
                                    <Button
                                        type="button"
                                        variant="ghost"
                                        size="sm"
                                        className="h-[22px] px-[8px] text-[11px]"
                                        aria-label={t("sshHostKey.copyFingerprint")}
                                        onClick={copyFingerprint}
                                    >
                                        {t("sshHostKey.copyFingerprint")}
                                    </Button>
                                </div>
                                <p
                                    data-testid="ssh-host-key-fingerprint"
                                    className="break-all font-mono text-(--ink-1)"
                                >
                                    {current.fingerprint}
                                </p>
                            </div>
                        </>
                    ) : (
                        <div>
                            <div className="mb-[4px] flex items-center justify-between gap-[8px]">
                                <p className="text-[11px] font-medium uppercase tracking-[0.04em] text-(--ink-3)">
                                    {t("sshHostKey.fingerprintLabel")}
                                </p>
                                <Button
                                    type="button"
                                    variant="ghost"
                                    size="sm"
                                    className="h-[22px] px-[8px] text-[11px]"
                                    aria-label={t("sshHostKey.copyFingerprint")}
                                    onClick={copyFingerprint}
                                >
                                    {t("sshHostKey.copyFingerprint")}
                                </Button>
                            </div>
                            <p
                                data-testid="ssh-host-key-fingerprint"
                                className="break-all font-mono text-(--ink-1)"
                            >
                                {current.fingerprint}
                            </p>
                        </div>
                    )}
                    {error ? <p className="text-[12px] text-[#b4232a]">{error}</p> : null}
                </div>
                <AlertDialogFooter>
                    {isNew ? (
                        <>
                            <AlertDialogCancel>{t("sshHostKey.reject")}</AlertDialogCancel>
                            <AlertDialogAction
                                onClick={(event) => {
                                    event.preventDefault()
                                    void acceptCurrent()
                                }}
                            >
                                {t("sshHostKey.accept")}
                            </AlertDialogAction>
                        </>
                    ) : (
                        <AlertDialogCancel>{t("sshHostKey.close")}</AlertDialogCancel>
                    )}
                </AlertDialogFooter>
            </AlertDialogContent>
        </AlertDialog>
    )
}

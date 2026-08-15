import { useEffect, useState } from "react"
import { listen } from "@tauri-apps/api/event"
import { useTranslation } from "react-i18next"

import { askpassRespond } from "../lib/ipc"
import type { AskpassKind, AskpassOperation, AskpassRequest } from "../lib/types"
import { useOverlayPresence } from "../state/overlayStore"
import {
    Dialog,
    DialogContent,
    DialogDescription,
    DialogFooter,
    DialogHeader,
    DialogTitle
} from "../components/ui/dialog"
import { Button } from "../components/ui/button"
import { Input } from "../components/ui/input"
import { ScrollArea } from "@/components/ui/scroll-area"

// Multi-line prompts (fingerprint/other) are shown verbatim; single-line
// prompts (username/password/passphrase) get an input field. Everything that
// isn't a known interactive text kind falls back to the trust/cancel layout.
const TEXT_KINDS: AskpassKind[] = ["username", "password", "passphrase"]
const MASKED_KINDS: AskpassKind[] = ["password", "passphrase"]

function subtitleFor(kind: AskpassKind, t: (key: string) => string): string {
    switch (kind) {
        case "username":
            return t("askpass.subtitleUsername")
        case "password":
            return t("askpass.subtitlePassword")
        case "passphrase":
            return t("askpass.subtitlePassphrase")
        case "fingerprint":
            return t("askpass.subtitleFingerprint")
        default:
            return t("askpass.subtitleOther")
    }
}

function operationLabel(operation: AskpassOperation, t: (key: string) => string): string {
    switch (operation) {
        case "fetch":
            return t("askpass.operationFetch")
        case "pull":
            return t("askpass.operationPull")
        case "push":
            return t("askpass.operationPush")
        case "probe":
            return t("askpass.operationProbe")
    }
}

function ContextRow({ label, value }: { label: string; value: string }) {
    return (
        <div>
            <p className="mb-[4px] text-[11px] font-medium uppercase tracking-[0.04em] text-(--ink-3)">
                {label}
            </p>
            <p className="break-all font-mono text-[12.5px] text-(--ink-1)">{value}</p>
        </div>
    )
}

export function AskpassHost() {
    const { t } = useTranslation("workbench")
    // queue[0] is the currently displayed request; the rest wait their turn.
    const [queue, setQueue] = useState<AskpassRequest[]>([])
    const [value, setValue] = useState("")

    useEffect(() => {
        const unlisten = listen<AskpassRequest>("git:askpass-request", (e) => {
            setQueue((q) => [...q, e.payload])
        })
        return () => {
            void unlisten.then((fn) => fn())
        }
    }, [])

    const current = queue[0] ?? null

    // A background git op can pop this dialog at any time; the preview child
    // webview (a native layer) must hide so it can't paint over it and freeze
    // the app (z-order gate). Runs before the early return to keep hook order stable.
    useOverlayPresence(current !== null)

    // Clear the input whenever the displayed request changes so a value typed
    // for one prompt can never leak into the next.
    useEffect(() => {
        setValue("")
    }, [current?.id])

    function advance(id: number, response: string | null) {
        void askpassRespond(id, response)
        setQueue((q) => q.filter((r) => r.id !== id))
    }

    if (!current) return null

    const isText = TEXT_KINDS.includes(current.kind)
    const masked = MASKED_KINDS.includes(current.kind)

    function submit() {
        if (!current) return
        advance(current.id, value)
    }

    function trust() {
        if (!current) return
        advance(current.id, "yes")
    }

    function cancel() {
        if (!current) return
        advance(current.id, null)
    }

    return (
        <Dialog
            open
            onOpenChange={(open) => {
                if (!open) cancel()
            }}
        >
            <DialogContent
                resizeId="askpass"
                showCloseButton={false}
                className="flex min-h-0 flex-col"
            >
                <DialogHeader>
                    <DialogTitle>{t("askpass.title")}</DialogTitle>
                    <DialogDescription>{subtitleFor(current.kind, t)}</DialogDescription>
                </DialogHeader>
                <div className="grid gap-[8px]">
                    <ContextRow
                        label={t("askpass.repositoryLabel")}
                        value={`${current.repositoryDisplay} (${current.repositoryCanonical})`}
                    />
                    <ContextRow
                        label={t("askpass.operationLabel")}
                        value={operationLabel(current.operation, t)}
                    />
                    <ContextRow
                        label={t("askpass.remoteLabel")}
                        value={current.remoteDisplay ?? t("askpass.remoteUnknown")}
                    />
                    <ContextRow
                        label={t("askpass.policyLabel")}
                        value={
                            current.background
                                ? t("askpass.policyBackground")
                                : t("askpass.policyForeground")
                        }
                    />
                </div>
                <p className="text-[11px] text-(--ink-3)">{t("askpass.untrustedPromptNote")}</p>
                {isText ? (
                    <div className="flex flex-col gap-[8px]">
                        <div className="text-[12.5px] whitespace-pre-wrap break-words text-(--ink-2)">
                            {current.prompt}
                        </div>
                        <Input
                            aria-label={t("askpass.inputLabel")}
                            type={masked ? "password" : "text"}
                            autoFocus
                            value={value}
                            onChange={(e) => setValue(e.target.value)}
                            onKeyDown={(e) => {
                                if (e.key === "Enter") {
                                    e.preventDefault()
                                    submit()
                                }
                            }}
                        />
                    </div>
                ) : (
                    <ScrollArea className="max-h-[320px] rounded-[10px] border border-(--line-1) bg-(--paper-2)" focusable>
                        <pre className="p-[10px] text-[12px] whitespace-pre-wrap break-words">
                            {current.prompt}
                        </pre>
                    </ScrollArea>
                )}
                <DialogFooter>
                    <Button variant="outline" onClick={cancel}>
                        {t("askpass.cancel")}
                    </Button>
                    {isText ? (
                        <Button onClick={submit}>{t("askpass.submit")}</Button>
                    ) : (
                        <Button onClick={trust}>{t("askpass.trust")}</Button>
                    )}
                </DialogFooter>
            </DialogContent>
        </Dialog>
    )
}

import { useEffect, useState } from "react"
import { listen } from "@tauri-apps/api/event"

import { askpassRespond } from "../lib/ipc"
import type { AskpassKind, AskpassRequest } from "../lib/types"
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

// Multi-line prompts (fingerprint/other) are shown verbatim; single-line
// prompts (username/password/passphrase) get an input field. Everything that
// isn't a known interactive text kind falls back to the trust/cancel layout.
const TEXT_KINDS: AskpassKind[] = ["username", "password", "passphrase"]
const MASKED_KINDS: AskpassKind[] = ["password", "passphrase"]

function subtitleFor(kind: AskpassKind): string {
    switch (kind) {
        case "username":
            return "Username"
        case "password":
            return "Password 或 token"
        case "passphrase":
            return "SSH key passphrase"
        case "fingerprint":
            return "Host 驗證"
        default:
            return "Host 驗證"
    }
}

export function AskpassHost() {
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
            <DialogContent showCloseButton={false} className="sm:max-w-[440px]">
                <DialogHeader>
                    <DialogTitle>Git 認證</DialogTitle>
                    <DialogDescription>{subtitleFor(current.kind)}</DialogDescription>
                </DialogHeader>
                {isText ? (
                    <div className="flex flex-col gap-[8px]">
                        <div className="text-[12.5px] whitespace-pre-wrap break-words text-(--ink-2)">
                            {current.prompt}
                        </div>
                        <Input
                            aria-label="認證輸入"
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
                    <pre className="max-h-[320px] overflow-auto rounded-[10px] border border-(--line-1) bg-(--paper-2) p-[10px] text-[12px] whitespace-pre-wrap break-words">
                        {current.prompt}
                    </pre>
                )}
                <DialogFooter>
                    <Button variant="outline" onClick={cancel}>
                        取消
                    </Button>
                    {isText ? (
                        <Button onClick={submit}>送出</Button>
                    ) : (
                        <Button onClick={trust}>信任並繼續</Button>
                    )}
                </DialogFooter>
            </DialogContent>
        </Dialog>
    )
}

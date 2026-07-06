import { useEffect, useMemo, useState } from "react"
import { ExternalLink } from "lucide-react"
import { openUrl } from "@tauri-apps/plugin-opener"

type FrameLoadState = "idle" | "loading" | "confirmed" | "unverified" | "timeout"

interface PreviewFrameProps {
    url: string | null
    reloadNonce: number
    timeoutMs?: number
}

function validatedLocalhostUrl(rawUrl: string | null): string | null {
    if (!rawUrl) return null
    try {
        const url = new URL(rawUrl)
        if (url.protocol !== "http:") return null
        if (url.hostname !== "localhost" && url.hostname !== "127.0.0.1") return null
        return rawUrl
    } catch {
        return null
    }
}

export function PreviewFrame({ url, reloadNonce, timeoutMs = 4500 }: PreviewFrameProps) {
    const safeUrl = useMemo(() => validatedLocalhostUrl(url), [url])
    const [loadState, setLoadState] = useState<FrameLoadState>("idle")

    useEffect(() => {
        if (!safeUrl) {
            setLoadState("idle")
            return
        }
        setLoadState("loading")
        const id = window.setTimeout(() => {
            setLoadState((state) => (state === "loading" ? "timeout" : state))
        }, timeoutMs)
        return () => window.clearTimeout(id)
    }, [safeUrl, reloadNonce, timeoutMs])

    if (url && !safeUrl) {
        return (
            <div className="flex min-h-0 flex-1 items-center justify-center p-[18px] text-center text-[12.5px] text-(--ink-3)">
                Preview 只允許 localhost 或 127.0.0.1。
            </div>
        )
    }

    if (!safeUrl) {
        return null
    }

    const openExternally = () => {
        void openUrl(safeUrl).catch(() => {})
    }

    const onLoad = (event: React.SyntheticEvent<HTMLIFrameElement>) => {
        let confirmed = false
        try {
            const frame = event.currentTarget
            const doc = frame.contentDocument ?? frame.contentWindow?.document ?? null
            confirmed = !!doc
        } catch {
            confirmed = false
        }
        setLoadState(confirmed ? "confirmed" : "unverified")
    }

    return (
        <div className="relative flex min-h-0 flex-1 flex-col">
            <iframe
                key={`${safeUrl}:${reloadNonce}`}
                title="Live preview"
                src={safeUrl}
                onLoad={onLoad}
                className="min-h-0 flex-1 border-0 bg-white"
                sandbox="allow-downloads allow-forms allow-modals allow-popups allow-popups-to-escape-sandbox allow-presentation allow-same-origin allow-scripts"
            />
            <div className="flex min-h-[30px] shrink-0 items-center justify-between gap-[10px] border-t border-(--line-1) bg-(--paper-1) px-[10px] py-[6px] text-[11px] text-(--ink-3)">
                <span>
                    {loadState === "timeout"
                        ? "載入逾時；頁面可能透過 X-Frame-Options 或 CSP frame-ancestors 禁止嵌入。"
                        : loadState === "unverified"
                          ? "無法確認 iframe 是否完成載入；若畫面空白，可能被 X-Frame-Options 或 CSP frame-ancestors 擋下。"
                          : "iframe preview"}
                </span>
                <button
                    type="button"
                    onClick={openExternally}
                    className="flex h-[22px] shrink-0 items-center gap-[5px] rounded-[6px] border border-(--line-1) bg-(--paper-0) px-[7px] text-[11px] text-(--ink-2) hover:bg-(--paper-2)"
                >
                    <ExternalLink className="size-[12px]" aria-hidden="true" />
                    Open externally
                </button>
            </div>
        </div>
    )
}

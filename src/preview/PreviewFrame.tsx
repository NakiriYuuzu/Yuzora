import { useMemo } from "react"

interface PreviewFrameProps {
    url: string | null
    reloadNonce: number
}

// PreviewPanel only mounts this for local dev-server / static-server URLs (external
// https goes to the child webview); this guard is defence in depth.
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

// The dev-server info + "Open externally" that used to live in a footer here moved
// to the StatusBar / PreviewPanel toolbar (they were duplicated), so this is just
// the iframe now.
export function PreviewFrame({ url, reloadNonce }: PreviewFrameProps) {
    const safeUrl = useMemo(() => validatedLocalhostUrl(url), [url])

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

    return (
        <iframe
            key={`${safeUrl}:${reloadNonce}`}
            title="Live preview"
            src={safeUrl}
            className="min-h-0 flex-1 border-0 bg-white"
            sandbox="allow-downloads allow-forms allow-modals allow-popups allow-popups-to-escape-sandbox allow-presentation allow-same-origin allow-scripts"
        />
    )
}

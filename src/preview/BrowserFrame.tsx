import { useTranslation } from "react-i18next"

interface BrowserFrameProps {
    url: string | null
    reloadNonce: number
}

// Remote loopback pages use their owned tunnel; external pages use the native webview.
export function BrowserFrame({ url, reloadNonce }: BrowserFrameProps) {
    const { t } = useTranslation("preview")
    if (!url) return null
    try {
        const parsed = new URL(url)
        if (parsed.protocol !== "http:" || !["localhost", "127.0.0.1"].includes(parsed.hostname)) return null
    } catch {
        return null
    }
    return (
        <iframe
            key={`${url}:${reloadNonce}`}
            title={t("browserFrameTitle")}
            src={url}
            className="min-h-0 flex-1 border-0 bg-white"
            sandbox="allow-downloads allow-forms allow-modals allow-popups allow-popups-to-escape-sandbox allow-presentation allow-same-origin allow-scripts"
        />
    )
}

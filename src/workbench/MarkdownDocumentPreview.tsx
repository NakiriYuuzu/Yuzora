import { memo, useEffect, useMemo, useRef, useState } from "react"
import { useTranslation } from "react-i18next"
import { openUrl } from "@tauri-apps/plugin-opener"
import { ScrollArea } from "@/components/ui/scroll-area"
import { MarkdownPreviewProse, sanitizeMarkdownHtml } from "./MarkdownPreview"
import { renderMarkdownDocument, type MarkdownHtmlBatch } from "./markdownDocumentRender"

const ReadingBatch = memo(function ReadingBatch({ batch, initialVisible }: { batch: MarkdownHtmlBatch; initialVisible: boolean }) {
    const ref = useRef<HTMLDivElement>(null)
    const [visible, setVisible] = useState(initialVisible || typeof IntersectionObserver === "undefined")
    const [height, setHeight] = useState(Math.max(32, batch.estimatedLines * 24))
    useEffect(() => {
        const element = ref.current
        if (!element) return
        if (typeof IntersectionObserver === "undefined") return
        const observer = new IntersectionObserver(([entry]) => setVisible(entry.isIntersecting), {
            root: element.closest('[data-slot="scroll-area-viewport"]'), rootMargin: "800px 0px",
        })
        observer.observe(element)
        return () => observer.disconnect()
    }, [])
    useEffect(() => {
        const element = ref.current
        if (!visible || !element) return
        const measure = () => {
            const measured = element.getBoundingClientRect().height
            if (measured > 0) setHeight(measured)
        }
        measure()
        const observer = new ResizeObserver(measure)
        observer.observe(element)
        return () => observer.disconnect()
    }, [visible])
    // Sanitize before mounting, retaining the exact shared preview contract.
    const html = useMemo(() => visible ? sanitizeMarkdownHtml(batch.html, false) : "", [batch.html, visible])
    return <div ref={ref} data-markdown-batch="" data-code-continuation={batch.code || undefined}
        style={visible ? { display: "flow-root" } : { height }}>
        {visible && <div dangerouslySetInnerHTML={{ __html: html }} />}
    </div>
})

/** Continuous document reading: parse off-thread, mount only nearby HTML. */
export function MarkdownDocumentPreview({ content }: { content: string }) {
    const { t } = useTranslation("markdownDocument")
    const [result, setResult] = useState<{ content: string; batches: MarkdownHtmlBatch[]; error?: boolean } | null>(null)
    useEffect(() => {
        let active = true
        let worker: Worker | undefined
        const done = (batches: MarkdownHtmlBatch[]) => { if (active) setResult({ content, batches }) }
        const failed = () => { if (active) setResult({ content, batches: [], error: true }) }
        try {
            if (content.length <= 128 * 1024) done(renderMarkdownDocument(content))
            else {
                worker = new Worker(new URL("./markdownDocument.worker.ts", import.meta.url), { type: "module" })
                worker.onmessage = (event: MessageEvent<MarkdownHtmlBatch[]>) => { done(event.data); worker?.terminate() }
                worker.onerror = (event) => { event.preventDefault(); failed(); worker?.terminate() }
                worker.postMessage(content)
            }
        } catch { failed() }
        return () => { active = false; worker?.terminate() }
    }, [content])
    const current = result?.content === content ? result : null
    return <ScrollArea className="min-h-0 min-w-0 flex-1" orientation="both" focusable
        viewportClassName="markdown-preview-body px-[28px] py-[20px] text-[14px] leading-[1.7] text-(--ink-1)"
        viewportProps={{ "data-testid": "markdown-document-preview", onClick: (event) => {
            const anchor = (event.target as HTMLElement).closest("a")
            if (!anchor) return
            event.preventDefault()
            const href = anchor.getAttribute("href") ?? ""
            if (/^https?:\/\//i.test(href)) void openUrl(href).catch(() => {})
        } }}>
        <MarkdownPreviewProse />
        <style>{`[data-code-continuation] pre{margin:0;border-radius:0;padding-top:0;padding-bottom:0}[data-code-continuation] code{white-space:pre-wrap;overflow-wrap:anywhere}`}</style>
        {!current ? <p role="status">{t("loading")}</p> : current.error ? <p role="alert">{t("error")}</p>
            : current.batches.length === 0 ? <p>{t("empty")}</p>
                : <div key={content}>{current.batches.map((batch, index) => <ReadingBatch key={index} batch={batch} initialVisible={index === 0} />)}</div>}
    </ScrollArea>
}

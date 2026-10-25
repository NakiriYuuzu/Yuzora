import { memo, useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState } from "react"
import type { KeyboardEvent } from "react"
import { ChevronDown, ChevronUp, FileLock2, FileWarning, FileX2, ZoomIn, ZoomOut } from "lucide-react"
import { useTranslation } from "react-i18next"
import type { PDFDocumentProxy, PDFPageProxy, RenderTask } from "pdfjs-dist/legacy/build/pdf.mjs"

import { EmptyState } from "@/app/workbench/EmptyState"
import { Button } from "@/components/ui/button"
import { Input } from "@/components/ui/input"
import { ScrollArea } from "@/components/ui/scroll-area"
import { workspacePathBasename, workspacePathForDisplay } from "@/lib/paths"
import { useWorkspaceStore } from "@/state/workspaceStore"
import { isPasswordError, isRenderCancelled, openPdfDocument, readPdfBytes, type PdfDocumentHandle } from "./pdfDocument"
import {
    PDF_TO_CSS_UNITS,
    ZOOM_STEP,
    anchorScrollTop,
    canvasOutputScale,
    clampZoom,
    currentPageIndex,
    fitWidthZoom,
    layoutPages,
    pageScrollTop,
    scrollAnchor,
    visiblePageRange,
    type PdfLayout,
    type PdfPageSize
} from "./pdfLayout"
import { retainedViewState, useRetainedScroll } from "./viewRetention"

export function isPdfPath(name: string): boolean {
    const ext = name.includes(".") ? name.split(".").pop()!.toLowerCase() : ""
    return ext === "pdf"
}

type LoadState =
    | { kind: "loading" }
    | { kind: "ready"; handle: PdfDocumentHandle }
    | { kind: "tooLarge"; size: number | null; limit: number }
    | { kind: "password" }
    | { kind: "error" }

interface ViewportMetrics {
    width: number
    height: number
    scrollTop: number
    scrollLeft: number
}

function formatBytes(size: number): string {
    if (size < 1024) return `${size} B`
    if (size < 1024 * 1024) return `${(size / 1024).toFixed(1)} KB`
    return `${(size / (1024 * 1024)).toFixed(1)} MB`
}

function naturalPageSize(page: PDFPageProxy): PdfPageSize {
    const viewport = page.getViewport({ scale: PDF_TO_CSS_UNITS })
    return { width: viewport.width, height: viewport.height }
}

/**
 * Read-only PDF reader. The file is read as bytes through the same IPC as
 * remote images, so local and SSH workspaces behave alike; `generation`
 * (bumped on external rewrites) reloads it in place, keeping page and zoom.
 */
export function PdfView({ path, generation }: { path: string; generation?: number }) {
    const { t } = useTranslation("pdfViewer")
    const [state, setState] = useState<LoadState>({ kind: "loading" })
    const [sizes, setSizes] = useState<PdfPageSize[]>([])
    // The handle most recently handed to React, released on unmount even if
    // that state update never committed.
    const handedOffRef = useRef<PdfDocumentHandle | null>(null)
    useEffect(() => () => handedOffRef.current?.destroy(), [])

    useEffect(() => {
        let disposed = false
        void (async () => {
            try {
                const file = await readPdfBytes(path)
                if (disposed) return
                if (file.kind === "tooLarge") {
                    setState({ kind: "tooLarge", size: file.size, limit: file.limit })
                    return
                }
                const handle = await openPdfDocument(file.bytes)
                let first: PdfPageSize
                try {
                    first = naturalPageSize(await handle.doc.getPage(1))
                } catch (error) {
                    handle.destroy()
                    throw error
                }
                if (disposed) {
                    handle.destroy()
                    return
                }
                handedOffRef.current = handle
                // Until a page is fetched for rendering, it is assumed to match page 1.
                setSizes(Array.from({ length: handle.doc.numPages }, () => first))
                setState({ kind: "ready", handle })
            } catch (error) {
                if (disposed) return
                const password = isPasswordError(error)
                // A rewrite caught mid-write does not parse yet: keep the open
                // document (and its page and zoom); the write's next change reloads.
                setState((current) => current.kind === "ready" && !password
                    ? current
                    : password ? { kind: "password" } : { kind: "error" })
            }
        })()
        return () => {
            disposed = true
        }
    }, [path, generation])

    // A reload swaps the handle; the previous document is released only after
    // the new one is committed, so its canvases stay up until then.
    const handle = state.kind === "ready" ? state.handle : null
    useEffect(() => () => handle?.destroy(), [handle])

    if (state.kind === "ready") {
        return <PdfReader path={path} doc={state.handle.doc} sizes={sizes} setSizes={setSizes} />
    }

    const name = workspacePathBasename(path)
    return (
        <div data-testid="pdf-view" className="flex min-h-0 min-w-0 flex-1 items-center justify-center">
            {state.kind === "loading" ? (
                <p aria-busy="true" className="text-[12.5px] text-(--ink-3)">{t("loading")}</p>
            ) : state.kind === "tooLarge" ? (
                <EmptyState
                    icon={FileWarning}
                    title={t("tooLargeTitle")}
                    description={state.size === null
                        ? t("tooLargeDescriptionUnknownSize", { name, limit: formatBytes(state.limit) })
                        : t("tooLargeDescription", { name, size: formatBytes(state.size), limit: formatBytes(state.limit) })}
                />
            ) : state.kind === "password" ? (
                <EmptyState icon={FileLock2} title={t("passwordTitle")} description={t("passwordDescription")} />
            ) : (
                <EmptyState
                    icon={FileX2}
                    title={t("loadError")}
                    description={t("loadErrorDescription", { path: workspacePathForDisplay(path) })}
                />
            )}
        </div>
    )
}

function PdfReader({
    path,
    doc,
    sizes,
    setSizes
}: {
    path: string
    doc: PDFDocumentProxy
    sizes: PdfPageSize[]
    setSizes: (update: (current: PdfPageSize[]) => PdfPageSize[]) => void
}) {
    const { t } = useTranslation("pdfViewer")
    const viewportRef = useRef<HTMLDivElement>(null)
    const workspacePath = useWorkspaceStore((s) => s.workspacePath)
    const zoomState = useMemo(
        () => retainedViewState<number | "fit-width">(workspacePath, path, "pdf-zoom"),
        [workspacePath, path]
    )
    // Comes back to the zoom (or fit-width mode) the document was last left at.
    const [zoom, setZoomState] = useState<number | "fit-width">(() => zoomState.get() ?? "fit-width")
    const setZoom = (next: number | "fit-width") => {
        zoomState.set(next)
        setZoomState(next)
    }
    const [metrics, setMetrics] = useState<ViewportMetrics>({ width: 0, height: 0, scrollTop: 0, scrollLeft: 0 })
    const [pageDraft, setPageDraft] = useState<string | null>(null)

    const effectiveZoom = zoom === "fit-width" ? fitWidthZoom(sizes[0]?.width ?? 0, metrics.width) : zoom
    const layout = useMemo(() => layoutPages(sizes, effectiveZoom), [sizes, effectiveZoom])
    const pageCount = sizes.length
    // The page last navigated to stays current until the reader scrolls away,
    // including where the end of the document keeps it from reaching the top.
    const [pinned, setPinned] = useState<{ page: number; scrollTop: number } | null>(null)
    const currentPage = pinned && pinned.scrollTop === metrics.scrollTop
        ? Math.min(pinned.page, pageCount - 1)
        : currentPageIndex(layout, metrics.scrollTop, metrics.height)
    const [firstVisible, lastVisible] = visiblePageRange(layout, metrics.scrollTop, metrics.height)

    // Mirrors the latest measurement for the relayout effect below.
    const metricsRef = useRef(metrics)

    // Measured before paint so the first page renders already at fit width.
    useLayoutEffect(() => {
        const viewport = viewportRef.current
        if (!viewport) return
        let frame = 0
        let hidden = false
        const read = () => {
            frame = 0
            const current = metricsRef.current
            // Hidden with the editor surface (display: none) in another mode:
            // keep the last real measurement, and restore the scroll position
            // the engine may have reset once it is shown again.
            if (viewport.clientWidth === 0 && viewport.clientHeight === 0) {
                hidden = true
                return
            }
            if (hidden) {
                hidden = false
                viewport.scrollTop = current.scrollTop
                viewport.scrollLeft = current.scrollLeft
            }
            const next = {
                width: viewport.clientWidth,
                height: viewport.clientHeight,
                scrollTop: viewport.scrollTop,
                scrollLeft: viewport.scrollLeft
            }
            if (current.width === next.width && current.height === next.height
                && current.scrollTop === next.scrollTop && current.scrollLeft === next.scrollLeft) return
            metricsRef.current = next
            setMetrics(next)
        }
        const schedule = () => {
            if (!frame) frame = requestAnimationFrame(read)
        }
        read()
        viewport.addEventListener("scroll", schedule, { passive: true })
        const observer = new ResizeObserver(schedule)
        observer.observe(viewport)
        return () => {
            viewport.removeEventListener("scroll", schedule)
            observer.disconnect()
            if (frame) cancelAnimationFrame(frame)
        }
    }, [])

    // Zoom, pane resizes (fit width) and newly measured page sizes all relayout;
    // keep the same spot of the same page under the reader.
    const previousLayoutRef = useRef<PdfLayout>(layout)
    useLayoutEffect(() => {
        const previous = previousLayoutRef.current
        previousLayoutRef.current = layout
        const viewport = viewportRef.current
        if (!viewport || previous === layout || previous.tops.length === 0) return
        const { scrollTop, scrollLeft, width } = metricsRef.current
        viewport.scrollTop = anchorScrollTop(layout, scrollAnchor(previous, scrollTop))
        viewport.scrollLeft = previous.contentWidth > 0
            ? Math.max(0, (scrollLeft + width / 2) * (layout.contentWidth / previous.contentWidth) - width / 2)
            : 0
        metricsRef.current = { ...metricsRef.current, scrollTop: viewport.scrollTop, scrollLeft: viewport.scrollLeft }
        setMetrics(metricsRef.current)
        setPinned((pin) => pin && pin.scrollTop === scrollTop ? { ...pin, scrollTop: viewport.scrollTop } : pin)
    }, [layout])

    // The scroll offset only means something once the pane is measured, so the
    // layout (page tops at the restored zoom, fit width included) is final.
    useRetainedScroll(
        viewportRef,
        { workspacePath, path, facet: "pdf-scroll" },
        pageCount > 0 && metrics.width > 0
    )

    const zoomBy = (factor: number) => setZoom(clampZoom(effectiveZoom * factor))
    // Non-passive so Ctrl/Cmd+wheel zooms instead of scrolling (as in ImageView).
    const zoomByRef = useRef(zoomBy)
    useLayoutEffect(() => {
        zoomByRef.current = zoomBy
    })
    useEffect(() => {
        const viewport = viewportRef.current
        if (!viewport) return
        const onWheel = (event: globalThis.WheelEvent) => {
            if (!event.ctrlKey && !event.metaKey) return
            event.preventDefault()
            zoomByRef.current(event.deltaY < 0 ? ZOOM_STEP : 1 / ZOOM_STEP)
        }
        viewport.addEventListener("wheel", onWheel, { passive: false })
        return () => viewport.removeEventListener("wheel", onWheel)
    }, [])

    function goToPage(index: number) {
        const viewport = viewportRef.current
        if (!viewport || pageCount === 0) return
        const page = Math.min(pageCount - 1, Math.max(0, index))
        viewport.scrollTop = pageScrollTop(layout, page)
        // Pin to the position as clamped by the engine, which the next scroll read sees.
        metricsRef.current = { ...metricsRef.current, scrollTop: viewport.scrollTop }
        setMetrics(metricsRef.current)
        setPinned({ page, scrollTop: viewport.scrollTop })
    }

    function commitPageDraft() {
        if (pageDraft === null) return
        const page = Number.parseInt(pageDraft, 10)
        setPageDraft(null)
        if (Number.isFinite(page)) goToPage(page - 1)
    }

    function onPageKeyDown(event: KeyboardEvent<HTMLInputElement>) {
        if (event.key === "Enter") commitPageDraft()
        else if (event.key === "Escape") setPageDraft(null)
    }

    const onPageSize = useCallback((index: number, size: PdfPageSize) => {
        setSizes((current) => {
            const known = current[index]
            if (!known || (Math.abs(known.width - size.width) < 0.5 && Math.abs(known.height - size.height) < 0.5)) {
                return current
            }
            const next = current.slice()
            next[index] = size
            return next
        })
    }, [setSizes])

    const pages = []
    for (let index = firstVisible; index <= lastVisible; index++) {
        pages.push(
            <PdfPage
                key={index}
                doc={doc}
                index={index}
                top={layout.tops[index]}
                left={(layout.contentWidth - layout.widths[index]) / 2}
                width={layout.widths[index]}
                height={layout.heights[index]}
                zoom={effectiveZoom}
                onSize={onPageSize}
            />
        )
    }

    return (
        <div data-testid="pdf-view" className="flex min-h-0 min-w-0 flex-1 flex-col">
            <div
                role="toolbar"
                aria-label={t("toolbar")}
                className="flex h-[32px] shrink-0 items-center gap-[4px] border-b border-(--line-1) bg-(--paper-0) px-[8px] text-[11.5px] text-(--ink-3)"
            >
                <Button
                    variant="ghost"
                    size="icon-xs"
                    aria-label={t("previousPage")}
                    title={t("previousPage")}
                    disabled={currentPage <= 0}
                    onClick={() => goToPage(currentPage - 1)}
                >
                    <ChevronUp aria-hidden="true" />
                </Button>
                <Button
                    variant="ghost"
                    size="icon-xs"
                    aria-label={t("nextPage")}
                    title={t("nextPage")}
                    disabled={currentPage >= pageCount - 1}
                    onClick={() => goToPage(currentPage + 1)}
                >
                    <ChevronDown aria-hidden="true" />
                </Button>
                <Input
                    aria-label={t("pageNumber")}
                    inputMode="numeric"
                    value={pageDraft ?? String(currentPage + 1)}
                    onFocus={(event) => event.currentTarget.select()}
                    onChange={(event) => setPageDraft(event.currentTarget.value)}
                    onKeyDown={onPageKeyDown}
                    onBlur={commitPageDraft}
                    className="h-6 w-[46px] px-1 text-center font-mono text-[11.5px] md:text-[11.5px]"
                />
                <span data-testid="pdf-view-page-count" className="font-mono whitespace-nowrap">
                    {t("pageCount", { count: pageCount })}
                </span>
                <span className="flex-1" />
                <Button
                    variant="ghost"
                    size="icon-xs"
                    aria-label={t("zoomOut")}
                    title={t("zoomOut")}
                    onClick={() => zoomBy(1 / ZOOM_STEP)}
                >
                    <ZoomOut aria-hidden="true" />
                </Button>
                <span
                    data-testid="pdf-view-zoom"
                    aria-label={t("zoomLevel")}
                    className="min-w-[40px] text-center font-mono"
                >
                    {Math.round(effectiveZoom * 100)}%
                </span>
                <Button
                    variant="ghost"
                    size="icon-xs"
                    aria-label={t("zoomIn")}
                    title={t("zoomIn")}
                    onClick={() => zoomBy(ZOOM_STEP)}
                >
                    <ZoomIn aria-hidden="true" />
                </Button>
                <Button
                    variant={zoom === "fit-width" ? "secondary" : "ghost"}
                    size="xs"
                    aria-pressed={zoom === "fit-width"}
                    onClick={() => setZoom("fit-width")}
                >
                    {t("fitWidth")}
                </Button>
            </div>
            <ScrollArea
                className="min-h-0 min-w-0 flex-1 bg-(--paper-2)"
                orientation="both"
                focusable
                viewportRef={viewportRef}
            >
                <div
                    data-testid="pdf-view-pages"
                    className="relative mx-auto"
                    style={{ width: layout.contentWidth, height: layout.totalHeight }}
                >
                    {pages}
                </div>
            </ScrollArea>
        </div>
    )
}

function releaseCanvases(host: HTMLElement) {
    for (const canvas of host.querySelectorAll("canvas")) {
        // WebKit frees the backing store only once the canvas is zero-sized.
        canvas.width = 0
        canvas.height = 0
    }
    host.replaceChildren()
}

/**
 * One page box. Only pages near the viewport are mounted. A new canvas is
 * swapped in once it finishes rendering, so zoom and reload never flash blank.
 */
const PdfPage = memo(function PdfPage({
    doc,
    index,
    top,
    left,
    width,
    height,
    zoom,
    onSize
}: {
    doc: PDFDocumentProxy
    index: number
    top: number
    left: number
    width: number
    height: number
    zoom: number
    onSize: (index: number, size: PdfPageSize) => void
}) {
    const { t } = useTranslation("pdfViewer")
    const hostRef = useRef<HTMLDivElement>(null)
    const pageRef = useRef<PDFPageProxy | null>(null)
    const [failed, setFailed] = useState(false)

    useEffect(() => {
        const host = hostRef.current
        if (!host) return
        let cancelled = false
        let task: RenderTask | null = null
        // Deferred a frame: a burst of zoom or resize steps renders only the last one.
        const frame = requestAnimationFrame(() => {
            void doc.getPage(index + 1)
                .then((page) => {
                    if (cancelled) return
                    pageRef.current = page
                    onSize(index, naturalPageSize(page))
                    const viewport = page.getViewport({ scale: zoom * PDF_TO_CSS_UNITS })
                    const scale = canvasOutputScale(viewport.width, viewport.height, window.devicePixelRatio || 1)
                    const canvas = document.createElement("canvas")
                    canvas.width = Math.max(1, Math.floor(viewport.width * scale))
                    canvas.height = Math.max(1, Math.floor(viewport.height * scale))
                    canvas.className = "absolute inset-0 size-full"
                    task = page.render({
                        canvas,
                        viewport,
                        transform: scale === 1 ? undefined : [scale, 0, 0, scale, 0, 0]
                    })
                    return task.promise.then(() => {
                        if (cancelled) {
                            canvas.width = 0
                            canvas.height = 0
                            return
                        }
                        releaseCanvases(host)
                        host.appendChild(canvas)
                        setFailed(false)
                    })
                })
                .catch((error: unknown) => {
                    if (!cancelled && !isRenderCancelled(error)) setFailed(true)
                })
        })
        return () => {
            cancelled = true
            cancelAnimationFrame(frame)
            task?.cancel()
        }
    }, [doc, index, zoom, onSize])

    // Scrolled out of range: drop the bitmap and pdf.js' per-page resources.
    useEffect(() => {
        const host = hostRef.current
        return () => {
            if (host) releaseCanvases(host)
            pageRef.current?.cleanup()
        }
    }, [])

    return (
        <div
            data-testid="pdf-view-page"
            data-page-number={index + 1}
            className="absolute bg-white shadow-[0_1px_3px_rgba(0,0,0,0.18)]"
            style={{ top, left, width, height }}
        >
            <div ref={hostRef} className="absolute inset-0" />
            {failed && (
                <p className="absolute inset-0 flex items-center justify-center text-[12px] text-neutral-500">
                    {t("pageRenderError")}
                </p>
            )}
        </div>
    )
})

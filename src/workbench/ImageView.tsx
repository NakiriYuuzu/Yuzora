import { useEffect, useMemo, useRef, useState } from "react"
import { ImageOff } from "lucide-react"
import { useTranslation } from "react-i18next"
import { convertFileSrc } from "@tauri-apps/api/core"

import { EmptyState } from "@/app/workbench/EmptyState"
import { openFile } from "@/lib/ipc"
import { workspacePathBasename, workspacePathForDisplay } from "@/lib/paths"

// Binary raster formats the WebView can decode natively. SVG is intentionally
// absent: it is a text file and opens as an editor with SvgSplitView.
const IMAGE_EXTENSIONS = new Set(["png", "jpg", "jpeg", "gif", "webp", "bmp", "ico", "avif"])

export function isImagePath(name: string): boolean {
    const ext = name.includes(".") ? name.split(".").pop()!.toLowerCase() : ""
    return IMAGE_EXTENSIONS.has(ext)
}

const MIN_ZOOM = 0.1
const MAX_ZOOM = 8
const ZOOM_STEP = 1.25

function clampZoom(zoom: number): number {
    return Math.min(MAX_ZOOM, Math.max(MIN_ZOOM, zoom))
}

function formatBytes(size: number): string {
    if (size < 1024) return `${size} B`
    if (size < 1024 * 1024) return `${(size / 1024).toFixed(1)} KB`
    return `${(size / (1024 * 1024)).toFixed(1)} MB`
}

// Checkerboard backdrop so transparency reads as transparency, not as the
// panel background. Light/dark variants follow the app theme class.
const CHECKERBOARD_CLASS =
    "[background:repeating-conic-gradient(rgba(0,0,0,0.07)_0%_25%,transparent_0%_50%)_0_0/16px_16px] " +
    "dark:[background:repeating-conic-gradient(rgba(255,255,255,0.07)_0%_25%,transparent_0%_50%)_0_0/16px_16px]"

export function ImageView({ path }: { path: string }) {
    const { t } = useTranslation("panels")
    const containerRef = useRef<HTMLDivElement>(null)
    // "fit" scales down to the viewport (never past 1:1); a number is an
    // explicit zoom factor from the wheel / toolbar.
    const [zoom, setZoom] = useState<number | "fit">("fit")
    const [dims, setDims] = useState<{ w: number; h: number } | null>(null)
    const [byteSize, setByteSize] = useState<number | null>(null)
    const [containerSize, setContainerSize] = useState<{ w: number; h: number } | null>(null)
    const [loadError, setLoadError] = useState(false)

    const src = useMemo(() => convertFileSrc(path), [path])
    const name = workspacePathBasename(path)

    // File size comes from the existing open_file metadata path (binary kind
    // carries size without reading content). Best-effort: on failure the
    // status bar simply omits the size.
    useEffect(() => {
        let disposed = false
        setByteSize(null)
        void openFile(path)
            .then((result) => {
                if (!disposed && typeof result.size === "number") setByteSize(result.size)
            })
            .catch(() => {})
        return () => {
            disposed = true
        }
    }, [path])

    useEffect(() => {
        const container = containerRef.current
        if (!container) return
        const update = (rect: Pick<DOMRectReadOnly, "width" | "height">) => {
            setContainerSize((current) =>
                current && current.w === rect.width && current.h === rect.height
                    ? current
                    : { w: rect.width, h: rect.height }
            )
        }
        update(container.getBoundingClientRect())
        const observer = new ResizeObserver((entries) => {
            const entry = entries[0]
            if (entry) update(entry.contentRect)
        })
        observer.observe(container)
        return () => observer.disconnect()
    }, [])

    // Fit never upscales: small images sit at 1:1, large ones shrink to the
    // viewport (with a little padding).
    const fitZoom =
        dims && containerSize
            ? Math.min(
                  1,
                  Math.max(MIN_ZOOM, (containerSize.w - 24) / dims.w),
                  Math.max(MIN_ZOOM, (containerSize.h - 24) / dims.h)
              )
            : 1
    const effectiveZoom = zoom === "fit" ? fitZoom : zoom

    function zoomBy(factor: number) {
        setZoom(clampZoom(effectiveZoom * factor))
    }

    // React (v17+) attaches wheel listeners passively at the root, so an
    // onWheel prop cannot preventDefault the container's own scrolling —
    // attach a non-passive listener directly. zoomBy goes through a ref so
    // the listener always sees the current zoom without re-attaching.
    const zoomByRef = useRef(zoomBy)
    zoomByRef.current = zoomBy
    useEffect(() => {
        const container = containerRef.current
        if (!container) return
        const onWheel = (event: globalThis.WheelEvent) => {
            if (!event.ctrlKey && !event.metaKey) return
            event.preventDefault()
            zoomByRef.current(event.deltaY < 0 ? ZOOM_STEP : 1 / ZOOM_STEP)
        }
        container.addEventListener("wheel", onWheel, { passive: false })
        return () => container.removeEventListener("wheel", onWheel)
    }, [loadError])

    if (loadError) {
        return (
            <div
                data-testid="image-view"
                className="flex min-h-0 min-w-0 flex-1 items-center justify-center"
            >
                <EmptyState
                    icon={ImageOff}
                    title={t("imageViewer.loadError")}
                    description={t("imageViewer.loadErrorDescription", {
                        path: workspacePathForDisplay(path)
                    })}
                />
            </div>
        )
    }

    const zoomPercent = Math.round(effectiveZoom * 100)
    const zoomButtonClass =
        "flex h-[22px] min-w-[24px] items-center justify-center rounded-[6px] px-[6px] text-[11.5px] " +
        "text-(--ink-3) transition-colors hover:bg-(--paper-3) hover:text-(--ink-0)"

    return (
        <div data-testid="image-view" className="flex min-h-0 min-w-0 flex-1 flex-col">
            <div
                ref={containerRef}
                className={`flex min-h-0 min-w-0 flex-1 items-center justify-center overflow-auto ${CHECKERBOARD_CLASS}`}
            >
                <img
                    src={src}
                    alt={name}
                    draggable={false}
                    data-testid="image-view-img"
                    className="block max-w-none select-none"
                    style={
                        dims
                            ? {
                                  width: Math.max(1, Math.round(dims.w * effectiveZoom)),
                                  height: Math.max(1, Math.round(dims.h * effectiveZoom))
                              }
                            : undefined
                    }
                    onLoad={(event) => {
                        const img = event.currentTarget
                        setLoadError(false)
                        setDims({ w: img.naturalWidth, h: img.naturalHeight })
                    }}
                    onError={() => setLoadError(true)}
                />
            </div>
            <div className="flex h-[26px] shrink-0 items-center gap-[10px] border-t border-(--line-1) bg-(--paper-0) px-[10px] font-mono text-[11px] text-(--ink-3)">
                <span data-testid="image-view-meta" className="truncate">
                    {dims ? `${dims.w}×${dims.h}` : "—"}
                    {byteSize !== null ? ` · ${formatBytes(byteSize)}` : ""}
                    {` · ${zoomPercent}%`}
                </span>
                <span className="flex-1" />
                <button
                    type="button"
                    className={zoomButtonClass}
                    aria-label={t("imageViewer.zoomOut")}
                    title={t("imageViewer.zoomOut")}
                    onClick={() => zoomBy(1 / ZOOM_STEP)}
                >
                    −
                </button>
                <button
                    type="button"
                    className={zoomButtonClass}
                    aria-label={t("imageViewer.zoomActual")}
                    title={t("imageViewer.zoomActual")}
                    onClick={() => setZoom(1)}
                >
                    1:1
                </button>
                <button
                    type="button"
                    className={zoomButtonClass}
                    aria-label={t("imageViewer.zoomIn")}
                    title={t("imageViewer.zoomIn")}
                    onClick={() => zoomBy(ZOOM_STEP)}
                >
                    ＋
                </button>
                <button
                    type="button"
                    className={zoomButtonClass}
                    aria-label={t("imageViewer.zoomFit")}
                    title={t("imageViewer.zoomFit")}
                    onClick={() => setZoom("fit")}
                >
                    {t("imageViewer.zoomFit")}
                </button>
            </div>
        </div>
    )
}

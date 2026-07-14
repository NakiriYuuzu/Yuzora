import { useEffect, useRef, useState } from "react"
import type { KeyboardEvent, PointerEvent } from "react"
import { FileWarning } from "lucide-react"
import { useTranslation } from "react-i18next"

import { EmptyState } from "@/app/workbench/EmptyState"
import { SplitRatioIndicator } from "@/app/workbench/SplitRatioIndicator"
import { getDocument } from "@/editor/documentRegistry"
import { getView } from "@/editor/viewRegistry"
import { EditorPane } from "@/editor/EditorPane"
import { useSvgPreviewStore } from "@/state/svgPreviewStore"
import { useWorkbenchLayoutStore } from "@/state/workbenchLayoutStore"
import { useUiStore } from "@/state/uiStore"
import { useWorkspaceStore } from "@/state/workspaceStore"
import {
    NARROW_BREAKPOINT,
    effectiveRatio,
    ratioBounds,
    ratioFromPointer,
    ratioText,
    type Orientation
} from "./splitMath"

// Store 本體住在 state/svgPreviewStore（見該檔說明）；由此 re-export 維持
// 「preview 開關跟著 preview 元件走」的既有 import 慣例（TabBar 等）。
export { useSvgPreviewStore } from "@/state/svgPreviewStore"

export function isSvgPath(name: string): boolean {
    const ext = name.includes(".") ? name.split(".").pop()!.toLowerCase() : ""
    return ext === "svg"
}

function bufferSvgContent(path: string, fallback: string): string {
    const live = getView(path)?.state.doc.toString()
    return live !== undefined ? live : fallback
}

/**
 * Companion SVG preview pane. Renders the live buffer through a blob URL fed
 * to an <img> — static-image mode, scripts never execute (constraint C8).
 * Live updates poll the CM6 doc identity every 400ms, the same mechanism and
 * rationale as MarkdownPreview (R4-3: identity check skips full toString when
 * nothing changed; an update-listener extension is not injectable from here).
 */
function SvgPreview({ path, style }: { path: string; style?: React.CSSProperties }) {
    const { t } = useTranslation("panels")
    const [content, setContent] = useState<string | null>(null)
    const [loadError, setLoadError] = useState(false)
    const [renderError, setRenderError] = useState(false)
    const lastDocRef = useRef<unknown>(null)

    useEffect(() => {
        let disposed = false
        setLoadError(false)
        setRenderError(false)
        setContent(null)
        void getDocument(path)
            .then((entry) => {
                if (disposed) return
                const fallback =
                    entry.result.kind === "full" ||
                    entry.result.kind === "limited" ||
                    entry.result.kind === "nonUtf8Readonly"
                        ? entry.result.content
                        : ""
                lastDocRef.current = getView(path)?.state.doc ?? null
                setContent(bufferSvgContent(path, fallback))
            })
            .catch(() => {
                if (!disposed) setLoadError(true)
            })
        return () => {
            disposed = true
        }
    }, [path])

    useEffect(() => {
        const id = setInterval(() => {
            const doc = getView(path)?.state.doc
            if (doc === undefined || doc === lastDocRef.current) return
            lastDocRef.current = doc
            const live = doc.toString()
            setRenderError(false)
            setContent((current) => (current === live ? current : live))
        }, 400)
        return () => clearInterval(id)
    }, [path])

    // Blob URL creation is a side effect with a paired revoke, so it lives in
    // an effect rather than useMemo: StrictMode's double-invoked memo would
    // leak one URL per content change. Each effect run revokes exactly the URL
    // it created; an <img> that already loaded is unaffected by the revoke.
    const [url, setUrl] = useState<string | null>(null)
    useEffect(() => {
        if (content === null) {
            setUrl(null)
            return
        }
        const next = URL.createObjectURL(new Blob([content], { type: "image/svg+xml" }))
        setUrl(next)
        return () => URL.revokeObjectURL(next)
    }, [content])

    return (
        <div
            data-testid="svg-preview"
            style={style}
            className="flex min-h-0 min-w-0 items-center justify-center overflow-auto border-l border-(--line-1) bg-(--paper-1) p-[12px]"
        >
            {loadError || renderError ? (
                <EmptyState
                    icon={FileWarning}
                    title={t("svgPreview.renderError")}
                    description={t("svgPreview.renderErrorDescription")}
                />
            ) : url ? (
                <img
                    src={url}
                    alt={path.split("/").pop() ?? path}
                    draggable={false}
                    data-testid="svg-preview-img"
                    className="max-h-full max-w-full select-none"
                    onError={() => setRenderError(true)}
                />
            ) : null}
        </div>
    )
}

interface Size {
    width: number
    height: number
}

interface DragState {
    pointerId: number
    rect: DOMRect
    orientation: Orientation
    initialRatio: number
    ratio: number
    moved: boolean
    canCommit: boolean
}

/**
 * SVG source editor + auto-open companion preview, mirroring
 * MarkdownSplitView's layout contract: same narrow-breakpoint orientation
 * flip, same divider interaction, and the same global
 * markdownEditorRatio preference (one split preference across companion
 * previews, per plan t3-3a).
 */
export function SvgSplitView({ path, groupIndex }: { path: string; groupIndex: number }) {
    const { t: tMenus } = useTranslation("menus")
    const { t: tWorkbench } = useTranslation("workbench")
    const containerRef = useRef<HTMLDivElement>(null)
    const dragRef = useRef<DragState | null>(null)
    const [size, setSize] = useState<Size>({ width: 0, height: 0 })
    const [transientRatio, setTransientRatio] = useState<number | null>(null)

    const mode = useUiStore((state) => state.mode)
    const activeGroupIndex = useWorkspaceStore((state) => state.activeGroupIndex)
    const previewOpen = useSvgPreviewStore((state) => !state.closedPaths[path])
    const storedRatio = useWorkbenchLayoutStore((state) => state.markdownEditorRatio)
    const setStoredRatio = useWorkbenchLayoutStore((state) => state.setMarkdownEditorRatio)

    useEffect(() => {
        const container = containerRef.current
        if (!container) return

        const updateSize = (next: Pick<DOMRectReadOnly, "width" | "height">) => {
            setSize((current) =>
                current.width === next.width && current.height === next.height
                    ? current
                    : { width: next.width, height: next.height }
            )
        }

        updateSize(container.getBoundingClientRect())
        const observer = new ResizeObserver((entries) => {
            const entry = entries[0]
            if (entry) updateSize(entry.contentRect)
        })
        observer.observe(container)
        return () => observer.disconnect()
    }, [])

    const orientation: Orientation = size.width < NARROW_BREAKPOINT ? "column" : "row"
    const axisSize = orientation === "row" ? size.width : size.height
    const ratio = effectiveRatio(transientRatio ?? storedRatio, axisSize)
    const bounds = ratioBounds(axisSize)
    const editorPercent = Math.round(ratio * 100)
    const editorLabel = tWorkbench("settings.sections.editor.label")
    const previewLabel = tWorkbench("settings.sections.preview.label")
    const valueText = ratioText(editorLabel, previewLabel, ratio)
    const showPreview = mode === "files" && groupIndex === activeGroupIndex && previewOpen

    function finishDrag(pointerId: number) {
        const drag = dragRef.current
        if (!drag || drag.pointerId !== pointerId) return
        dragRef.current = null
        setTransientRatio(null)
        if (drag.moved && drag.canCommit) setStoredRatio(drag.ratio)
    }

    function onPointerDown(event: PointerEvent<HTMLDivElement>) {
        if (event.button !== 0) return
        const container = containerRef.current
        if (!container) return
        const rect = container.getBoundingClientRect()
        const capturedAxisSize = orientation === "row" ? rect.width : rect.height
        dragRef.current = {
            pointerId: event.pointerId,
            rect,
            orientation,
            initialRatio: ratio,
            ratio,
            moved: false,
            canCommit: ratioBounds(capturedAxisSize).canResize
        }
        setTransientRatio(ratio)
        event.currentTarget.setPointerCapture(event.pointerId)
        event.preventDefault()
    }

    function onPointerMove(event: PointerEvent<HTMLDivElement>) {
        const drag = dragRef.current
        if (!drag || drag.pointerId !== event.pointerId) return
        const capturedAxisSize = drag.orientation === "row" ? drag.rect.width : drag.rect.height
        const next = effectiveRatio(
            ratioFromPointer(drag.rect, drag.orientation, event.clientX, event.clientY),
            capturedAxisSize
        )
        drag.ratio = next
        drag.moved = next !== drag.initialRatio
        setTransientRatio(next)
        event.preventDefault()
    }

    function onPointerUp(event: PointerEvent<HTMLDivElement>) {
        finishDrag(event.pointerId)
        if (event.currentTarget.hasPointerCapture(event.pointerId)) {
            event.currentTarget.releasePointerCapture(event.pointerId)
        }
    }

    function onKeyDown(event: KeyboardEvent<HTMLDivElement>) {
        if (dragRef.current || !bounds.canResize) return

        let direction = 0
        if (orientation === "row") {
            if (event.key === "ArrowLeft") direction = -1
            if (event.key === "ArrowRight") direction = 1
        } else {
            if (event.key === "ArrowUp") direction = -1
            if (event.key === "ArrowDown") direction = 1
        }
        if (direction === 0) return

        event.preventDefault()
        const step = event.shiftKey ? 0.1 : 0.02
        const next = effectiveRatio(ratio + direction * step, axisSize)
        if (next !== ratio) setStoredRatio(next)
    }

    return (
        <div
            ref={containerRef}
            data-testid="svg-split-view"
            data-orientation={orientation}
            className={`svg-split-view flex min-h-0 min-w-0 flex-1 ${
                orientation === "row" ? "flex-row" : "flex-col"
            }`}
        >
            <div
                data-testid="svg-editor-surface"
                className="flex min-h-0 min-w-0 overflow-hidden"
                style={{ flexBasis: 0, flexGrow: showPreview ? ratio : 1 }}
            >
                <EditorPane path={path} groupIndex={groupIndex} />
            </div>
            {showPreview && (
                <>
                    <div
                        role="separator"
                        tabIndex={0}
                        aria-label={tMenus("terminalDrawer.dragToResize")}
                        aria-orientation={orientation === "row" ? "vertical" : "horizontal"}
                        aria-valuemin={Math.round(bounds.min * 100)}
                        aria-valuemax={Math.round(bounds.max * 100)}
                        aria-valuenow={editorPercent}
                        aria-valuetext={valueText}
                        title={tMenus("terminalDrawer.dragToResize")}
                        data-testid="svg-preview-divider"
                        className={`relative shrink-0 touch-none bg-(--line-1) transition-colors hover:bg-(--yz-accent) focus-visible:bg-(--yz-accent) focus-visible:outline-none ${
                            orientation === "row"
                                ? "h-full w-[6px] cursor-col-resize"
                                : "h-[6px] w-full cursor-row-resize"
                        }`}
                        onPointerDown={onPointerDown}
                        onPointerMove={onPointerMove}
                        onPointerUp={onPointerUp}
                        onPointerCancel={(event) => finishDrag(event.pointerId)}
                        onLostPointerCapture={(event) => finishDrag(event.pointerId)}
                        onKeyDown={onKeyDown}
                    >
                        {transientRatio !== null && <SplitRatioIndicator text={valueText} />}
                    </div>
                    <SvgPreview path={path} style={{ flexBasis: 0, flexGrow: 1 - ratio }} />
                </>
            )}
        </div>
    )
}

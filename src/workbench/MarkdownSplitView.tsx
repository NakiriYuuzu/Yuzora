import { useEffect, useRef, useState } from "react"
import type { KeyboardEvent, PointerEvent } from "react"
import { useTranslation } from "react-i18next"

import { SplitRatioIndicator } from "@/app/workbench/SplitRatioIndicator"
import { useWorkbenchLayoutStore } from "@/state/workbenchLayoutStore"
import { useUiStore } from "@/state/uiStore"
import { useWorkspaceStore } from "@/state/workspaceStore"
import { EditorPane } from "@/editor/EditorPane"
import { MarkdownPreview, useMarkdownPreviewStore } from "./MarkdownPreview"

const NARROW_BREAKPOINT = 640
const MIN_PANE_SIZE = 160
const DIVIDER_SIZE = 6

type Orientation = "row" | "column"

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

function ratioBounds(axisSize: number) {
    const paneSpace = Math.max(0, axisSize - DIVIDER_SIZE)
    if (paneSpace <= MIN_PANE_SIZE * 2) {
        return { min: 0.5, max: 0.5, canResize: false }
    }
    const min = MIN_PANE_SIZE / paneSpace
    return { min, max: 1 - min, canResize: true }
}

function effectiveRatio(ratio: number, axisSize: number): number {
    const bounds = ratioBounds(axisSize)
    return Math.min(bounds.max, Math.max(bounds.min, ratio))
}

function ratioFromPointer(rect: DOMRect, orientation: Orientation, clientX: number, clientY: number) {
    const axisSize = orientation === "row" ? rect.width : rect.height
    const pointerOffset = orientation === "row" ? clientX - rect.left : clientY - rect.top
    const paneSpace = Math.max(0, axisSize - DIVIDER_SIZE)
    if (paneSpace === 0) return 0.5
    return (pointerOffset - DIVIDER_SIZE / 2) / paneSpace
}

function ratioText(firstLabel: string, secondLabel: string, firstRatio: number): string {
    const firstPercent = Math.round(firstRatio * 100)
    return `${firstLabel} ${firstPercent}% · ${secondLabel} ${100 - firstPercent}%`
}

export function MarkdownSplitView({ path, groupIndex }: { path: string; groupIndex: number }) {
    const { t: tMenus } = useTranslation("menus")
    const { t: tWorkbench } = useTranslation("workbench")
    const containerRef = useRef<HTMLDivElement>(null)
    const dragRef = useRef<DragState | null>(null)
    const [size, setSize] = useState<Size>({ width: 0, height: 0 })
    const [transientRatio, setTransientRatio] = useState<number | null>(null)

    const mode = useUiStore((state) => state.mode)
    const activeGroupIndex = useWorkspaceStore((state) => state.activeGroupIndex)
    const previewOpen = useMarkdownPreviewStore((state) => !!state.openPaths[path])
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
            data-testid="markdown-split-view"
            data-orientation={orientation}
            className={`markdown-split-view flex min-h-0 min-w-0 flex-1 ${
                orientation === "row" ? "flex-row" : "flex-col"
            }`}
        >
            <div
                data-testid="markdown-editor-surface"
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
                        data-testid="markdown-preview-divider"
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
                        {transientRatio !== null && (
                            <SplitRatioIndicator text={valueText} />
                        )}
                    </div>
                    <MarkdownPreview path={path} style={{ flexBasis: 0, flexGrow: 1 - ratio }} />
                </>
            )}
        </div>
    )
}

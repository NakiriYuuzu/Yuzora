import { fireEvent } from "@testing-library/react"

import type { DragPoint } from "@/lib/pointerDrag"

/**
 * jsdom has no layout, so `document.elementFromPoint` does not exist. Route
 * hit-tests through `resolve` and return a restore function for afterEach.
 */
export function stubElementFromPoint(resolve: (point: DragPoint) => Element | null): () => void {
    const original = document.elementFromPoint
    document.elementFromPoint = (x: number, y: number) => resolve({ x, y })
    return () => {
        if (original) document.elementFromPoint = original
        else delete (document as { elementFromPoint?: unknown }).elementFromPoint
    }
}

/**
 * Drives a pointer drag the way a WebView delivers it: pointerdown on the
 * handle, then window-level moves with the button held, then the release.
 * The first move crosses the activation threshold; the rest hover targets.
 */
export function pointerDrag(
    source: Element,
    path: readonly DragPoint[],
    { from = { x: 0, y: 0 }, pointerId = 1, release = true }: { from?: DragPoint; pointerId?: number; release?: boolean } = {}
): void {
    fireEvent.pointerDown(source, { button: 0, buttons: 1, pointerId, clientX: from.x, clientY: from.y })
    for (const point of path) {
        fireEvent.pointerMove(window, { button: -1, buttons: 1, pointerId, clientX: point.x, clientY: point.y })
    }
    const last = path.at(-1) ?? from
    if (release) fireEvent.pointerUp(window, { button: 0, buttons: 0, pointerId, clientX: last.x, clientY: last.y })
}

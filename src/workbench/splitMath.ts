// Shared split-pane math for the editor-group companion previews
// (MarkdownSplitView, SvgSplitView). Pure functions only — drag state and JSX
// stay with each view.

export const NARROW_BREAKPOINT = 640
const MIN_PANE_SIZE = 160
const DIVIDER_SIZE = 6

export type Orientation = "row" | "column"

export function ratioBounds(axisSize: number) {
    const paneSpace = Math.max(0, axisSize - DIVIDER_SIZE)
    if (paneSpace <= MIN_PANE_SIZE * 2) {
        return { min: 0.5, max: 0.5, canResize: false }
    }
    const min = MIN_PANE_SIZE / paneSpace
    return { min, max: 1 - min, canResize: true }
}

export function effectiveRatio(ratio: number, axisSize: number): number {
    const bounds = ratioBounds(axisSize)
    return Math.min(bounds.max, Math.max(bounds.min, ratio))
}

export function ratioFromPointer(
    rect: DOMRect,
    orientation: Orientation,
    clientX: number,
    clientY: number
) {
    const axisSize = orientation === "row" ? rect.width : rect.height
    const pointerOffset = orientation === "row" ? clientX - rect.left : clientY - rect.top
    const paneSpace = Math.max(0, axisSize - DIVIDER_SIZE)
    if (paneSpace === 0) return 0.5
    return (pointerOffset - DIVIDER_SIZE / 2) / paneSpace
}

export function ratioText(firstLabel: string, secondLabel: string, firstRatio: number): string {
    const firstPercent = Math.round(firstRatio * 100)
    return `${firstLabel} ${firstPercent}% · ${secondLabel} ${100 - firstPercent}%`
}

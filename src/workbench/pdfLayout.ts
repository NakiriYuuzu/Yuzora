// Pure page geometry for PdfView: continuous vertical layout, which page the
// reader is on, which pages are worth rendering, and zoom bounds.

// PDF user space is 1/72 in, CSS pixels are 1/96 in: 100% is physical size,
// matching pdf.js' own viewer.
export const PDF_TO_CSS_UNITS = 96 / 72
export const PAGE_GAP = 12
export const PAGE_PADDING = 16
export const MIN_ZOOM = 0.25
export const MAX_ZOOM = 5
export const ZOOM_STEP = 1.25
// Per-canvas pixel budget. Past it a page renders slightly soft instead of
// exhausting WebView canvas memory at high zoom on HiDPI screens.
export const MAX_CANVAS_PIXELS = 4096 * 4096

/** Page size in CSS pixels at 100%. */
export interface PdfPageSize {
    width: number
    height: number
}

export interface PdfLayout {
    tops: number[]
    widths: number[]
    heights: number[]
    totalHeight: number
    contentWidth: number
}

/** Where the reader is within a page, kept stable across zoom and relayout. */
export interface ScrollAnchor {
    page: number
    fraction: number
}

export function clampZoom(zoom: number): number {
    return Math.min(MAX_ZOOM, Math.max(MIN_ZOOM, zoom))
}

export function fitWidthZoom(pageWidth: number, viewportWidth: number): number {
    if (pageWidth <= 0 || viewportWidth <= 0) return 1
    return clampZoom((viewportWidth - 2 * PAGE_PADDING) / pageWidth)
}

export function layoutPages(sizes: readonly PdfPageSize[], zoom: number): PdfLayout {
    const tops: number[] = []
    const widths: number[] = []
    const heights: number[] = []
    let y = PAGE_PADDING
    let maxWidth = 0
    for (const size of sizes) {
        const width = Math.max(1, Math.floor(size.width * zoom))
        const height = Math.max(1, Math.floor(size.height * zoom))
        tops.push(y)
        widths.push(width)
        heights.push(height)
        maxWidth = Math.max(maxWidth, width)
        y += height + PAGE_GAP
    }
    return {
        tops,
        widths,
        heights,
        totalHeight: sizes.length ? y - PAGE_GAP + PAGE_PADDING : 0,
        contentWidth: maxWidth + 2 * PAGE_PADDING
    }
}

/** The last page whose top is at or above `offset`; 0 above the first page. */
export function pageIndexAt(layout: PdfLayout, offset: number): number {
    let low = 0
    let high = layout.tops.length - 1
    let found = 0
    while (low <= high) {
        const mid = (low + high) >> 1
        if (layout.tops[mid] <= offset) {
            found = mid
            low = mid + 1
        } else high = mid - 1
    }
    return found
}

/**
 * The page with the largest visible share of its height, the earliest on ties,
 * so a page scrolled to by pageScrollTop is always the current one. Once
 * scrolled to the end, the last page — a short final page may never lead.
 */
export function currentPageIndex(layout: PdfLayout, scrollTop: number, viewportHeight: number): number {
    const count = layout.tops.length
    if (count === 0) return 0
    if (scrollTop > 0 && scrollTop + viewportHeight >= layout.totalHeight - 1) return count - 1
    const bottom = scrollTop + viewportHeight
    let best = pageIndexAt(layout, scrollTop)
    let bestShare = -1
    for (let index = best; index < count && layout.tops[index] < bottom; index++) {
        const top = layout.tops[index]
        const visible = Math.min(bottom, top + layout.heights[index]) - Math.max(scrollTop, top)
        const share = visible / layout.heights[index]
        if (share > bestShare + 1e-6) {
            best = index
            bestShare = share
        }
    }
    return best
}

/** Inclusive range of pages within `overscan` viewports of the visible area. */
export function visiblePageRange(
    layout: PdfLayout,
    scrollTop: number,
    viewportHeight: number,
    overscan = 1
): [number, number] {
    if (layout.tops.length === 0) return [0, -1]
    const margin = viewportHeight * overscan
    return [
        pageIndexAt(layout, scrollTop - margin),
        pageIndexAt(layout, scrollTop + viewportHeight + margin)
    ]
}

export function pageScrollTop(layout: PdfLayout, index: number): number {
    return index <= 0 ? 0 : Math.max(0, layout.tops[index] - PAGE_GAP / 2)
}

export function scrollAnchor(layout: PdfLayout, scrollTop: number): ScrollAnchor {
    const page = pageIndexAt(layout, scrollTop)
    const span = layout.heights[page] + PAGE_GAP
    return { page, fraction: span > 0 ? (scrollTop - layout.tops[page]) / span : 0 }
}

export function anchorScrollTop(layout: PdfLayout, anchor: ScrollAnchor): number {
    if (layout.tops.length === 0) return 0
    const page = Math.min(anchor.page, layout.tops.length - 1)
    return Math.max(0, layout.tops[page] + anchor.fraction * (layout.heights[page] + PAGE_GAP))
}

/** Device pixels per CSS pixel for a page canvas, capped by MAX_CANVAS_PIXELS. */
export function canvasOutputScale(cssWidth: number, cssHeight: number, devicePixelRatio: number): number {
    const area = cssWidth * cssHeight
    if (area <= 0) return 1
    return Math.min(devicePixelRatio, Math.sqrt(MAX_CANVAS_PIXELS / area))
}

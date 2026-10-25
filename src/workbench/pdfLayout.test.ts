import { describe, expect, it } from "vitest"

import {
    MAX_CANVAS_PIXELS,
    MAX_ZOOM,
    MIN_ZOOM,
    PAGE_GAP,
    PAGE_PADDING,
    anchorScrollTop,
    canvasOutputScale,
    clampZoom,
    currentPageIndex,
    fitWidthZoom,
    layoutPages,
    pageIndexAt,
    pageScrollTop,
    scrollAnchor,
    visiblePageRange
} from "./pdfLayout"

const letter = { width: 816, height: 1056 }

describe("layoutPages", () => {
    it("stacks pages with padding and gaps at the given zoom", () => {
        const layout = layoutPages([letter, { width: 1056, height: 816 }, letter], 0.5)
        expect(layout.widths).toEqual([408, 528, 408])
        expect(layout.heights).toEqual([528, 408, 528])
        expect(layout.tops).toEqual([PAGE_PADDING, PAGE_PADDING + 528 + PAGE_GAP, PAGE_PADDING + 528 + 408 + 2 * PAGE_GAP])
        expect(layout.totalHeight).toBe(layout.tops[2] + 528 + PAGE_PADDING)
        expect(layout.contentWidth).toBe(528 + 2 * PAGE_PADDING)
    })

    it("is empty without pages", () => {
        expect(layoutPages([], 1)).toMatchObject({ tops: [], totalHeight: 0 })
    })
})

describe("page lookup", () => {
    const layout = layoutPages([letter, letter, letter, letter], 1)

    it("finds the page whose top is at or above an offset", () => {
        expect(pageIndexAt(layout, -100)).toBe(0)
        expect(pageIndexAt(layout, layout.tops[2])).toBe(2)
        expect(pageIndexAt(layout, layout.tops[2] - 1)).toBe(1)
        expect(pageIndexAt(layout, 1e9)).toBe(3)
    })

    it("tracks the most visible page, and the last page at the end", () => {
        expect(currentPageIndex(layout, 0, 900)).toBe(0)
        expect(currentPageIndex(layout, layout.tops[1] - 200, 900)).toBe(1)
        // Pages 3 and 4 both fit: the earlier one leads until the very end.
        const shortLast = layoutPages([letter, letter, letter, { width: 816, height: 200 }], 1)
        const end = shortLast.totalHeight - 1400
        expect(currentPageIndex(shortLast, end - 2, 1400)).toBe(2)
        expect(currentPageIndex(shortLast, end, 1400)).toBe(3)
    })

    it("keeps page 1 current when the whole document fits", () => {
        const short = layoutPages([letter, letter], 0.25)
        expect(currentPageIndex(short, 0, 2000)).toBe(0)
    })

    it("agrees with pageScrollTop on pages much shorter than the viewport", () => {
        // 16:9 slides at fit width in a tall, narrow pane.
        const slides = layoutPages(Array.from({ length: 10 }, () => ({ width: 1280, height: 720 })), fitWidthZoom(1280, 500))
        expect(currentPageIndex(slides, 0, 900)).toBe(0)
        for (let index = 1; index < 7; index++) {
            expect(currentPageIndex(slides, pageScrollTop(slides, index), 900)).toBe(index)
        }
    })

    it("widens the rendered range by the overscan", () => {
        const top = layout.tops[2]
        expect(visiblePageRange(layout, top, 500, 0)).toEqual([2, 2])
        expect(visiblePageRange(layout, top, 500, 1)).toEqual([1, 2])
        expect(visiblePageRange(layoutPages([], 1), 0, 500)).toEqual([0, -1])
    })

    it("scrolls to a page top just above its edge, and to 0 for the first page", () => {
        expect(pageScrollTop(layout, 0)).toBe(0)
        expect(pageScrollTop(layout, 2)).toBe(layout.tops[2] - PAGE_GAP / 2)
        expect(currentPageIndex(layout, pageScrollTop(layout, 2), 900)).toBe(2)
    })
})

describe("scroll anchors", () => {
    it("keeps the same spot of the same page across zoom", () => {
        const before = layoutPages([letter, letter, letter], 1)
        const scrollTop = before.tops[1] + before.heights[1] / 2
        const after = layoutPages([letter, letter, letter], 2)
        const restored = anchorScrollTop(after, scrollAnchor(before, scrollTop))
        expect(pageIndexAt(after, restored)).toBe(1)
        expect(restored - after.tops[1]).toBeCloseTo((after.heights[1] + PAGE_GAP) * ((before.heights[1] / 2) / (before.heights[1] + PAGE_GAP)))
    })

    it("clamps an anchor to the last page of a shorter document", () => {
        const layout = layoutPages([letter], 1)
        expect(anchorScrollTop(layout, { page: 5, fraction: 0 })).toBe(layout.tops[0])
    })
})

describe("zoom", () => {
    it("clamps zoom", () => {
        expect(clampZoom(0.01)).toBe(MIN_ZOOM)
        expect(clampZoom(99)).toBe(MAX_ZOOM)
        expect(clampZoom(1.5)).toBe(1.5)
    })

    it("fits the page width inside the padding", () => {
        expect(fitWidthZoom(800, 800 + 2 * PAGE_PADDING)).toBe(1)
        expect(fitWidthZoom(800, 400 + 2 * PAGE_PADDING)).toBe(0.5)
        expect(fitWidthZoom(0, 900)).toBe(1)
        expect(fitWidthZoom(800, 0)).toBe(1)
        expect(fitWidthZoom(800, 20)).toBe(MIN_ZOOM)
    })
})

describe("canvasOutputScale", () => {
    it("uses the device pixel ratio within the pixel budget", () => {
        expect(canvasOutputScale(800, 1000, 2)).toBe(2)
    })

    it("caps large canvases at the pixel budget", () => {
        const scale = canvasOutputScale(4000, 5000, 2)
        expect(scale).toBeLessThan(2)
        expect(4000 * scale * 5000 * scale).toBeCloseTo(MAX_CANVAS_PIXELS)
    })
})

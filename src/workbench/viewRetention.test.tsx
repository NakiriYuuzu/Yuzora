import { useRef } from "react"
import { cleanup, fireEvent, render, screen } from "@testing-library/react"
import { afterEach, describe, expect, it } from "vitest"

import { PREVIEW_TAB_PATH } from "@/state/workspaceStore"
import {
    clearRetainedViewStatesForTest,
    renameRetainedViewStates,
    retainedViewState,
    setRetainedFacetLimit,
    tabRetention,
    useRetainedScroll,
    type RetainedScrollPosition
} from "./viewRetention"

afterEach(() => {
    cleanup()
    clearRetainedViewStatesForTest()
})

describe("tabRetention", () => {
    it("keeps live pages alive and snapshots documents", () => {
        expect(tabRetention({ kind: "herdr-terminal", path: "yuzora://herdr/default/t1" })).toBe("keep-alive")
        expect(tabRetention({ kind: "preview", path: PREVIEW_TAB_PATH })).toBe("keep-alive")
        expect(tabRetention({ path: PREVIEW_TAB_PATH })).toBe("keep-alive")
        expect(tabRetention({ path: "/w/a.ts" })).toBe("snapshot")
        expect(tabRetention({ kind: "markdown-preview", path: "yuzora://markdown-preview/%2Fw%2Fa.md" })).toBe("snapshot")
    })
})

describe("view-state registry", () => {
    it("keys by workspace, document and facet", () => {
        retainedViewState<number>("/w", "/w/a.md", "one").set(1)
        retainedViewState<number>("/w", "/w/a.md", "two").set(2)
        expect(retainedViewState<number>("/w", "/w/a.md", "one").get()).toBe(1)
        expect(retainedViewState<number>("/w", "/w/a.md", "two").get()).toBe(2)
        expect(retainedViewState<number>("/other", "/w/a.md", "one").get()).toBeUndefined()
        expect(retainedViewState<number>("/w", "/w/b.md", "one").get()).toBeUndefined()
    })

    it("bounds each facet by recency without evicting other facets", () => {
        setRetainedFacetLimit("bounded", 2)
        retainedViewState<string>("/w", "a", "other").set("kept")
        retainedViewState<string>("/w", "a", "bounded").set("a")
        retainedViewState<string>("/w", "b", "bounded").set("b")
        retainedViewState<string>("/w", "a", "bounded").set("a again")
        retainedViewState<string>("/w", "c", "bounded").set("c")
        expect(retainedViewState("/w", "b", "bounded").get()).toBeUndefined()
        expect(retainedViewState("/w", "a", "bounded").get()).toBe("a again")
        expect(retainedViewState("/w", "c", "bounded").get()).toBe("c")
        expect(retainedViewState("/w", "a", "other").get()).toBe("kept")
    })

    it("bounds a weighted facet by total weight and never keeps an oversized entry", () => {
        setRetainedFacetLimit("weighted", 10, 100)
        retainedViewState<string>("/w", "a", "weighted").set("a", 60)
        retainedViewState<string>("/w", "b", "weighted").set("b", 30)
        retainedViewState<string>("/w", "c", "weighted").set("c", 30)
        expect(retainedViewState("/w", "a", "weighted").get()).toBeUndefined()
        expect(retainedViewState("/w", "b", "weighted").get()).toBe("b")
        expect(retainedViewState("/w", "c", "weighted").get()).toBe("c")

        retainedViewState<string>("/w", "huge", "weighted").set("huge", 101)
        expect(retainedViewState("/w", "huge", "weighted").get()).toBeUndefined()
        expect(retainedViewState("/w", "b", "weighted").get()).toBeUndefined()

        retainedViewState<string>("/w", "d", "weighted").set("d", 40)
        retainedViewState<string>("/w", "d", "weighted").delete()
        retainedViewState<string>("/w", "e", "weighted").set("e", 100)
        expect(retainedViewState("/w", "e", "weighted").get()).toBe("e")
    })

    it("moves every facet of a renamed document", () => {
        retainedViewState<number>("/w", "/w/old.md", "one").set(1)
        retainedViewState<number>("/w", "/w/old.md", "two").set(2)
        retainedViewState<number>("/other", "/w/old.md", "one").set(9)
        renameRetainedViewStates("/w", "/w/old.md", "/w/new.md")
        expect(retainedViewState("/w", "/w/old.md", "one").get()).toBeUndefined()
        expect(retainedViewState("/w", "/w/new.md", "one").get()).toBe(1)
        expect(retainedViewState("/w", "/w/new.md", "two").get()).toBe(2)
        expect(retainedViewState("/other", "/w/old.md", "one").get()).toBe(9)
    })
})

function Scroller({ ready = true, path = "/w/doc.md", canRestore }: {
    ready?: boolean
    path?: string
    canRestore?: () => boolean
}) {
    const ref = useRef<HTMLDivElement>(null)
    useRetainedScroll(ref, { workspacePath: "/w", path, facet: "test-scroll" }, ready, canRestore)
    return <div ref={ref} data-testid="scroller"><div /></div>
}

function clampScrollTop(maxTop: () => number): () => void {
    const tops = new WeakMap<Element, number>()
    Object.defineProperty(HTMLDivElement.prototype, "scrollTop", {
        configurable: true,
        get(this: Element) { return tops.get(this) ?? 0 },
        set(this: Element, value: number) { tops.set(this, Math.min(maxTop(), Math.max(0, value))) }
    })
    return () => { delete (HTMLDivElement.prototype as { scrollTop?: number }).scrollTop }
}

function saved(path = "/w/doc.md") {
    return retainedViewState<RetainedScrollPosition>("/w", path, "test-scroll").get()
}

describe("useRetainedScroll", () => {
    it("records scrolling and restores it on the next mount", () => {
        const first = render(<Scroller />)
        const element = screen.getByTestId("scroller")
        element.scrollTop = 120
        element.scrollLeft = 8
        fireEvent.scroll(element)
        expect(saved()).toEqual({ top: 120, left: 8 })
        first.unmount()

        render(<Scroller />)
        expect(screen.getByTestId("scroller").scrollTop).toBe(120)
        expect(screen.getByTestId("scroller").scrollLeft).toBe(8)
    })

    it("waits until the content is ready before restoring", () => {
        retainedViewState<RetainedScrollPosition>("/w", "/w/doc.md", "test-scroll").set({ top: 64, left: 0 })
        const view = render(<Scroller ready={false} />)
        expect(screen.getByTestId("scroller").scrollTop).toBe(0)
        view.rerender(<Scroller ready />)
        expect(screen.getByTestId("scroller").scrollTop).toBe(64)
    })

    it("keeps the target while the content is still too short, until the user scrolls", () => {
        retainedViewState<RetainedScrollPosition>("/w", "/w/doc.md", "test-scroll").set({ top: 500, left: 0 })
        let maxTop = 100
        const restore = clampScrollTop(() => maxTop)
        try {
            render(<Scroller />)
            const element = screen.getByTestId("scroller")
            expect(element.scrollTop).toBe(100)
            // The restore's own (clamped) scroll event must not replace the target.
            fireEvent.scroll(element)
            expect(saved()).toEqual({ top: 500, left: 0 })

            maxTop = 1000
            fireEvent.wheel(element)
            element.scrollTop = 300
            fireEvent.scroll(element)
            expect(saved()).toEqual({ top: 300, left: 0 })
        } finally {
            restore()
        }
    })

    it("ends the restore on user input anywhere, such as the scrollbar or another editor", () => {
        retainedViewState<RetainedScrollPosition>("/w", "/w/doc.md", "test-scroll").set({ top: 500, left: 0 })
        const restore = clampScrollTop(() => 100)
        try {
            render(<Scroller />)
            const element = screen.getByTestId("scroller")
            fireEvent.pointerDown(document.body)
            fireEvent.scroll(element)
            expect(saved()).toEqual({ top: 100, left: 0 })
        } finally {
            restore()
        }
    })

    it("does not restore while something else owns the position", () => {
        retainedViewState<RetainedScrollPosition>("/w", "/w/doc.md", "test-scroll").set({ top: 64, left: 0 })
        render(<Scroller canRestore={() => false} />)
        const element = screen.getByTestId("scroller")
        expect(element.scrollTop).toBe(0)
        element.scrollTop = 20
        fireEvent.scroll(element)
        expect(saved()).toEqual({ top: 20, left: 0 })
    })

    it("keeps documents apart", () => {
        const first = render(<Scroller path="/w/a.md" />)
        const element = screen.getByTestId("scroller")
        element.scrollTop = 40
        fireEvent.scroll(element)
        first.unmount()
        render(<Scroller path="/w/b.md" />)
        expect(screen.getByTestId("scroller").scrollTop).toBe(0)
    })
})

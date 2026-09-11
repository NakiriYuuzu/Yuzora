import { EditorState, StateEffect } from "@codemirror/state"
import { codeFolding, foldEffect } from "@codemirror/language"
import { EditorView } from "@codemirror/view"
import { afterEach, describe, expect, it, vi } from "vitest"
import { fireEvent, waitFor } from "@testing-library/react"
import { minimap, minimapViewportGeometry, minimapCompartment } from "./minimap"

const views: EditorView[] = []
afterEach(() => {
    views.splice(0).forEach((view) => view.destroy())
    document.body.replaceChildren()
    vi.unstubAllGlobals()
    vi.restoreAllMocks()
})

function editor() {
    const parent = document.createElement("div")
    document.body.append(parent)
    const view = new EditorView({ parent, state: EditorState.create({ doc: Array.from({ length: 100 }, (_, i) => `line ${i}`).join("\n"), extensions: [minimapCompartment.of(minimap(true))] }) })
    views.push(view)
    Object.defineProperties(view.scrollDOM, {
        clientHeight: { configurable: true, value: 200 },
        scrollHeight: { configurable: true, value: 1000 },
    })
    return view
}

describe("interactive editor minimap", () => {
    it("positions source bars by CodeMirror's visual blocks after folding instead of equal source-line spacing", async () => {
        const view = editor()
        view.dispatch({ effects: StateEffect.appendConfig.of(codeFolding()) })
        Object.defineProperty(view.scrollDOM, "scrollHeight", { configurable: true, get: () => Math.max(200, view.contentHeight) })
        const panel = view.dom.querySelector<HTMLElement>(".yz-minimap")!
        const bar = panel.querySelectorAll<HTMLElement>(".yz-minimap-bar")[79]
        const from = view.state.doc.line(80).from
        const oldTop = view.lineBlockAt(from).top
        view.dispatch({ effects: foldEffect.of({ from: view.state.doc.line(2).to, to: view.state.doc.line(60).to }) })
        expect(view.lineBlockAt(from).top).toBeLessThan(oldTop)
        await waitFor(() => expect(parseFloat(bar.style.top)).toBeCloseTo(
            (view.lineBlockAt(from).top + view.documentPadding.top) / view.scrollDOM.scrollHeight * 100,
        ))
        expect(panel.querySelectorAll<HTMLElement>(".yz-minimap-bar")[30].hidden).toBe(true)
    })

    it("clamps viewport geometry and measures the actual visible fraction, not CodeMirror's overscan range", () => {
        expect(minimapViewportGeometry(300, 1000, 200, 100)).toEqual({ top: 30, height: 20, maxScrollTop: 800 })
        expect(minimapViewportGeometry(-10, 1000, 200, 100).top).toBe(0)
        expect(minimapViewportGeometry(2000, 1000, 200, 100).top).toBe(80)
        expect(minimapViewportGeometry(0, 100, 200, 100)).toEqual({ top: 0, height: 100, maxScrollTop: 0 })
        expect(minimapViewportGeometry(0, 0, 0, 100)).toEqual({ top: 0, height: 100, maxScrollTop: 0 })
    })
    it("shows the actual viewport after scrolling and clicking changes scrollTop without moving selection", async () => {
        const view = editor()
        const panel = view.dom.querySelector<HTMLElement>(".yz-minimap")!
        const thumb = view.dom.querySelector<HTMLElement>(".yz-minimap-viewport")
        expect(thumb).not.toBeNull()
        Object.defineProperty(panel, "clientHeight", { configurable: true, value: 100 })
        panel.getBoundingClientRect = () => ({ left: 0, right: 64, top: 20, bottom: 120, width: 64, height: 100, x: 0, y: 20, toJSON: () => ({}) })
        view.scrollDOM.scrollTop = 300
        fireEvent.scroll(view.scrollDOM)
        await waitFor(() => expect(thumb!.style.top).toBe("30px"))
        expect(thumb!.style.height).toBe("20px")
        fireEvent.pointerDown(panel, { pointerId: 1, button: 0, clientY: 100 })
        expect(view.scrollDOM.scrollTop).toBe(700)
        expect(view.state.selection.main.head).toBe(0)
        fireEvent.pointerUp(panel, { pointerId: 1 })
    })

    it("drags from the grabbed viewport offset, clamps edges, and stops on cancellation", async () => {
        const view = editor()
        const panel = view.dom.querySelector<HTMLElement>(".yz-minimap")!
        Object.defineProperty(panel, "clientHeight", { configurable: true, value: 100 })
        panel.getBoundingClientRect = () => ({ top: 0, left: 0, width: 64, height: 100, right: 64, bottom: 100, x: 0, y: 0, toJSON: () => ({}) })
        view.scrollDOM.scrollTop = 300
        fireEvent.scroll(view.scrollDOM)
        await waitFor(() => expect(panel.getAttribute("aria-valuenow")).toBe("300"))
        fireEvent.pointerDown(panel, { pointerId: 1, button: 0, clientY: 35 })
        expect(view.scrollDOM.scrollTop).toBe(300)
        fireEvent.pointerMove(panel, { pointerId: 1, clientY: 55 })
        expect(view.scrollDOM.scrollTop).toBe(500)
        fireEvent.pointerMove(panel, { pointerId: 1, clientY: 500 })
        expect(view.scrollDOM.scrollTop).toBe(800)
        fireEvent.pointerCancel(panel, { pointerId: 1 })
        fireEvent.pointerMove(panel, { pointerId: 1, clientY: 0 })
        expect(view.scrollDOM.scrollTop).toBe(800)
        expect(view.state.selection.main.head).toBe(0)
        fireEvent.keyDown(panel, { key: "Home" })
        expect(view.scrollDOM.scrollTop).toBe(0)
        fireEvent.keyDown(panel, { key: "PageDown" })
        expect(view.scrollDOM.scrollTop).toBe(200)
    })

    it("updates on resize and document growth and removes observers/listeners when disabled", async () => {
        const observers: Array<{ callback: ResizeObserverCallback; targets: Element[]; disconnect: ReturnType<typeof vi.fn> }> = []
        vi.stubGlobal("ResizeObserver", class {
            targets: Element[] = []
            disconnect = vi.fn()
            constructor(public callback: ResizeObserverCallback) { observers.push(this) }
            observe(target: Element) { this.targets.push(target) }
            unobserve() {}
        })
        const view = editor()
        const panel = view.dom.querySelector<HTMLElement>(".yz-minimap")!
        Object.defineProperty(panel, "clientHeight", { configurable: true, value: 100 })
        const observer = observers.find((entry) => entry.targets.includes(panel))!
        observer.callback([], {} as ResizeObserver)
        await waitFor(() => expect(panel.querySelector<HTMLElement>(".yz-minimap-viewport")!.style.height).toBe("20px"))
        Object.defineProperty(view.scrollDOM, "clientHeight", { configurable: true, value: 400 })
        observer.callback([], {} as ResizeObserver)
        await waitFor(() => expect(panel.querySelector<HTMLElement>(".yz-minimap-viewport")!.style.height).toBe("40px"))
        Object.defineProperty(view.scrollDOM, "scrollHeight", { configurable: true, value: 2000 })
        view.dispatch({ changes: { from: view.state.doc.length, insert: "\nnew line" } })
        await waitFor(() => expect(panel.querySelectorAll(".yz-minimap-bar")).toHaveLength(101))
        expect(panel.querySelector<HTMLElement>(".yz-minimap-viewport")!.style.height).toBe("20px")
        const remove = vi.spyOn(view.scrollDOM, "removeEventListener")
        view.dispatch({ effects: minimapCompartment.reconfigure(minimap(false)) })
        expect(panel.isConnected).toBe(false)
        expect(observer.disconnect).toHaveBeenCalled()
        expect(remove).toHaveBeenCalledWith("scroll", expect.any(Function))
        fireEvent.pointerDown(panel, { pointerId: 1, button: 0, clientY: 90 })
        expect(view.scrollDOM.scrollTop).toBe(0)
    })
})

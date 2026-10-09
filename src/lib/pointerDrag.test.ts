import { fireEvent } from "@testing-library/react"
import { afterEach, beforeEach, describe, expect, test, vi } from "vitest"

import { beginPointerDrag, insertionSide, isPointerDragActive, type PointerDragOptions } from "./pointerDrag"
import { pointerDrag, stubElementFromPoint } from "@/test/pointerDrag"

const platform = vi.hoisted(() => ({ mac: false }))
vi.mock("@/lib/platform", () => ({ isMacPlatform: () => platform.mac }))

let source: HTMLButtonElement
let targetA: HTMLDivElement
let targetB: HTMLDivElement
let restoreHitTest: () => void

function rect(left: number, top: number, width: number, height: number): DOMRect {
    return { left, top, width, height, right: left + width, bottom: top + height, x: left, y: top, toJSON: () => ({}) } as DOMRect
}

function track(overrides: Partial<PointerDragOptions<string>> = {}) {
    const onDrop = vi.fn()
    const onStart = vi.fn()
    const onEnd = vi.fn()
    const options: PointerDragOptions<string> = {
        resolveTarget: (point) => {
            const hit = document.elementFromPoint(point.x, point.y)
            if (hit === targetA) return { element: targetA, data: "a" }
            if (hit === targetB) return { element: targetB, position: "before", data: "b" }
            return null
        },
        onDrop,
        onStart,
        onEnd,
        ...overrides,
    }
    source.addEventListener("pointerdown", (event) => { beginPointerDrag(event, options) })
    return { onDrop, onStart, onEnd }
}

beforeEach(() => {
    platform.mac = false
    document.body.innerHTML = ""
    source = document.createElement("button")
    targetA = document.createElement("div")
    targetB = document.createElement("div")
    document.body.append(source, targetA, targetB)
    restoreHitTest = stubElementFromPoint(({ x }) => (x >= 100 && x < 200 ? targetA : x >= 200 ? targetB : null))
})

afterEach(() => {
    restoreHitTest()
    vi.useRealTimers()
})

describe("beginPointerDrag", () => {
    test("a press below the threshold stays a click and never starts", () => {
        const { onDrop, onStart } = track()
        const onClick = vi.fn()
        source.addEventListener("click", onClick)
        pointerDrag(source, [{ x: 2, y: 2 }])
        fireEvent.click(source)
        expect(onStart).not.toHaveBeenCalled()
        expect(onDrop).not.toHaveBeenCalled()
        expect(onClick).toHaveBeenCalledTimes(1)
        expect(isPointerDragActive()).toBe(false)
    })

    test("past the threshold it marks source, root and target, then drops on the resolved target", () => {
        const { onDrop, onStart, onEnd } = track({ label: "file.ts" })
        pointerDrag(source, [{ x: 10, y: 0 }, { x: 150, y: 0 }], { release: false })
        expect(onStart).toHaveBeenCalledTimes(1)
        expect(source).toHaveAttribute("data-pointer-drag-source", "true")
        expect(document.documentElement).toHaveAttribute("data-pointer-dragging", "true")
        expect(targetA).toHaveAttribute("data-pointer-drop-target", "inside")
        expect(document.querySelector(".pointer-drag-preview")).toHaveTextContent("file.ts")

        fireEvent.pointerMove(window, { buttons: 1, pointerId: 1, clientX: 250, clientY: 0 })
        expect(targetA).not.toHaveAttribute("data-pointer-drop-target")
        expect(targetB).toHaveAttribute("data-pointer-drop-target", "before")

        fireEvent.pointerUp(window, { pointerId: 1, clientX: 250, clientY: 0 })
        expect(onDrop).toHaveBeenCalledWith(expect.objectContaining({ element: targetB, data: "b" }), { x: 250, y: 0 })
        expect(onEnd).toHaveBeenCalledTimes(1)
        expect(source).not.toHaveAttribute("data-pointer-drag-source")
        expect(targetB).not.toHaveAttribute("data-pointer-drop-target")
        expect(document.documentElement).not.toHaveAttribute("data-pointer-dragging")
        expect(document.querySelector(".pointer-drag-preview")).toBeNull()
        expect(isPointerDragActive()).toBe(false)
    })

    test("releasing over nothing ends the drag without a drop", () => {
        const { onDrop, onEnd } = track()
        pointerDrag(source, [{ x: 10, y: 0 }, { x: 50, y: 0 }])
        expect(onDrop).not.toHaveBeenCalled()
        expect(onEnd).toHaveBeenCalledTimes(1)
    })

    test("swallows only the click synthesized by the drop", () => {
        vi.useFakeTimers()
        track()
        const onClick = vi.fn()
        source.addEventListener("click", onClick)
        pointerDrag(source, [{ x: 10, y: 0 }, { x: 150, y: 0 }])
        fireEvent.click(source)
        expect(onClick).not.toHaveBeenCalled()
        vi.runAllTimers()
        fireEvent.click(source)
        expect(onClick).toHaveBeenCalledTimes(1)
    })

    test("Escape cancels an active drag, stays out of other handlers and swallows the release click", () => {
        vi.useFakeTimers()
        const { onDrop, onEnd } = track()
        const onKey = vi.fn()
        document.addEventListener("keydown", onKey)
        const onClick = vi.fn()
        source.addEventListener("click", onClick)
        pointerDrag(source, [{ x: 10, y: 0 }, { x: 150, y: 0 }], { release: false })
        fireEvent.keyDown(document, { key: "Escape" })
        expect(onKey).not.toHaveBeenCalled()
        expect(onEnd).toHaveBeenCalledTimes(1)
        expect(targetA).not.toHaveAttribute("data-pointer-drop-target")

        fireEvent.pointerUp(window, { pointerId: 1, clientX: 150, clientY: 0 })
        fireEvent.click(source)
        expect(onDrop).not.toHaveBeenCalled()
        expect(onClick).not.toHaveBeenCalled()
        vi.runAllTimers()
        fireEvent.click(source)
        expect(onClick).toHaveBeenCalledTimes(1)
        document.removeEventListener("keydown", onKey)
    })

    test("Escape before the threshold is left to the page", () => {
        track()
        const onKey = vi.fn()
        document.addEventListener("keydown", onKey)
        pointerDrag(source, [{ x: 1, y: 0 }], { release: false })
        fireEvent.keyDown(document, { key: "Escape" })
        expect(onKey).toHaveBeenCalledTimes(1)
        fireEvent.pointerUp(window, { pointerId: 1, clientX: 1, clientY: 0 })
        document.removeEventListener("keydown", onKey)
    })

    test("window blur and pointercancel end the drag without a drop", () => {
        const first = track()
        pointerDrag(source, [{ x: 10, y: 0 }, { x: 150, y: 0 }], { release: false })
        fireEvent.blur(window)
        fireEvent.pointerUp(window, { pointerId: 1, clientX: 150, clientY: 0 })
        expect(first.onDrop).not.toHaveBeenCalled()
        expect(first.onEnd).toHaveBeenCalledTimes(1)

        pointerDrag(source, [{ x: 10, y: 0 }, { x: 150, y: 0 }], { release: false, pointerId: 2 })
        fireEvent.pointerCancel(window, { pointerId: 2 })
        expect(isPointerDragActive()).toBe(false)
        expect(first.onDrop).not.toHaveBeenCalled()
    })

    test("a move without any button pressed means the release was lost", () => {
        const { onDrop, onEnd } = track()
        pointerDrag(source, [{ x: 10, y: 0 }, { x: 150, y: 0 }], { release: false })
        fireEvent.pointerMove(window, { buttons: 0, pointerId: 1, clientX: 160, clientY: 0 })
        expect(onEnd).toHaveBeenCalledTimes(1)
        expect(isPointerDragActive()).toBe(false)
        expect(onDrop).not.toHaveBeenCalled()
    })

    test("a press whose release was lost before the threshold never becomes a drag", () => {
        const { onDrop, onStart } = track()
        pointerDrag(source, [{ x: 1, y: 0 }], { release: false })
        fireEvent.pointerMove(window, { buttons: 0, pointerId: 1, clientX: 150, clientY: 0 })
        expect(onStart).not.toHaveBeenCalled()
        expect(isPointerDragActive()).toBe(false)
        expect(document.documentElement).not.toHaveAttribute("data-pointer-dragging")
        expect(targetA).not.toHaveAttribute("data-pointer-drop-target")
        fireEvent.pointerUp(window, { pointerId: 1, clientX: 150, clientY: 0 })
        expect(onDrop).not.toHaveBeenCalled()
    })

    test("events from another pointer are ignored", () => {
        const { onDrop } = track()
        pointerDrag(source, [{ x: 10, y: 0 }, { x: 150, y: 0 }], { release: false, pointerId: 7 })
        fireEvent.pointerUp(window, { pointerId: 8, clientX: 150, clientY: 0 })
        expect(isPointerDragActive()).toBe(true)
        fireEvent.pointerUp(window, { pointerId: 7, clientX: 150, clientY: 0 })
        expect(onDrop).toHaveBeenCalledTimes(1)
    })

    test("a refused pointer capture keeps the gesture on window listeners", () => {
        const capture = vi.spyOn(HTMLElement.prototype, "setPointerCapture").mockImplementation(() => {
            throw new DOMException("refused", "NotFoundError")
        })
        const { onDrop } = track()
        pointerDrag(source, [{ x: 10, y: 0 }, { x: 150, y: 0 }])
        expect(capture).toHaveBeenCalled()
        expect(onDrop).toHaveBeenCalledWith(expect.objectContaining({ data: "a" }), { x: 150, y: 0 })
        capture.mockRestore()
    })

    test("only the primary button starts and macOS Control-click never does", () => {
        const options = { resolveTarget: () => null, onDrop: vi.fn() }
        expect(beginPointerDrag({ button: 2, pointerId: 1, clientX: 0, clientY: 0, currentTarget: source }, options)).toBeNull()
        platform.mac = true
        expect(beginPointerDrag({ button: 0, ctrlKey: true, pointerId: 1, clientX: 0, clientY: 0, currentTarget: source }, options)).toBeNull()
        platform.mac = false
        const stop = beginPointerDrag({ button: 0, ctrlKey: true, pointerId: 1, clientX: 0, clientY: 0, currentTarget: source }, options)
        expect(stop).toBeTypeOf("function")
        stop?.()
        expect(isPointerDragActive()).toBe(false)
    })

    test("a new press replaces a gesture whose release never arrived", () => {
        const options = { resolveTarget: () => null, onDrop: vi.fn(), onEnd: vi.fn() }
        beginPointerDrag({ button: 0, pointerId: 1, clientX: 0, clientY: 0, currentTarget: source }, options)
        fireEvent.pointerMove(window, { buttons: 1, pointerId: 1, clientX: 20, clientY: 0 })
        fireEvent.pointerDown(window, { button: 0, pointerId: 2, clientX: 0, clientY: 0 })
        expect(options.onEnd).toHaveBeenCalledTimes(1)
        expect(isPointerDragActive()).toBe(false)
    })

    test("the disposer tears an active drag down", () => {
        const options = { resolveTarget: () => ({ element: targetA, data: "a" }), onDrop: vi.fn(), onEnd: vi.fn() }
        const stop = beginPointerDrag({ button: 0, pointerId: 1, clientX: 0, clientY: 0, currentTarget: source }, options)
        fireEvent.pointerMove(window, { buttons: 1, pointerId: 1, clientX: 20, clientY: 0 })
        expect(targetA).toHaveAttribute("data-pointer-drop-target")
        stop?.()
        expect(targetA).not.toHaveAttribute("data-pointer-drop-target")
        expect(options.onEnd).toHaveBeenCalledTimes(1)
        fireEvent.pointerUp(window, { pointerId: 1, clientX: 20, clientY: 0 })
        expect(options.onDrop).not.toHaveBeenCalled()
    })

    test("auto-scrolls a container while the pointer rests in its edge zone", () => {
        const frames: FrameRequestCallback[] = []
        const raf = vi.spyOn(window, "requestAnimationFrame").mockImplementation((callback) => {
            frames.push(callback)
            return frames.length
        })
        const scroller = document.createElement("div")
        document.body.append(scroller)
        Object.defineProperties(scroller, {
            scrollWidth: { value: 1000 },
            clientWidth: { value: 300 },
            scrollHeight: { value: 100 },
            clientHeight: { value: 100 },
        })
        vi.spyOn(scroller, "getBoundingClientRect").mockReturnValue(rect(0, 0, 300, 100))
        const resolveTarget = vi.fn(() => null)
        track({ resolveTarget, autoScroll: () => [scroller] })
        pointerDrag(source, [{ x: 10, y: 50 }, { x: 295, y: 50 }], { release: false })
        expect(frames).toHaveLength(1)
        frames.shift()?.(0)
        expect(scroller.scrollLeft).toBeGreaterThan(0)
        const afterFirst = scroller.scrollLeft
        frames.shift()?.(0)
        expect(scroller.scrollLeft).toBeGreaterThan(afterFirst)
        // Targets are re-resolved under the still pointer as content moves.
        expect(resolveTarget.mock.calls.length).toBeGreaterThanOrEqual(3)

        fireEvent.pointerMove(window, { buttons: 1, pointerId: 1, clientX: 150, clientY: 50 })
        const settled = scroller.scrollLeft
        frames.shift()?.(0)
        expect(scroller.scrollLeft).toBe(settled)
        fireEvent.pointerUp(window, { pointerId: 1, clientX: 150, clientY: 50 })
        raf.mockRestore()
    })

    test("auto-scrolls only the container under the pointer", () => {
        const frames: FrameRequestCallback[] = []
        const raf = vi.spyOn(window, "requestAnimationFrame").mockImplementation((callback) => {
            frames.push(callback)
            return frames.length
        })
        const strip = (left: number) => {
            const element = document.createElement("div")
            document.body.append(element)
            Object.defineProperties(element, {
                scrollWidth: { value: 1000 },
                clientWidth: { value: 300 },
                scrollHeight: { value: 30 },
                clientHeight: { value: 30 },
            })
            vi.spyOn(element, "getBoundingClientRect").mockReturnValue(rect(left, 0, 300, 30))
            return element
        }
        // Two editor groups' tab strips side by side; the right one is scrolled.
        const leftStrip = strip(0)
        const rightStrip = strip(300)
        rightStrip.scrollLeft = 200
        track({ resolveTarget: () => null, autoScroll: () => [leftStrip, rightStrip] })
        pointerDrag(source, [{ x: 10, y: 15 }, { x: 295, y: 15 }], { release: false })
        frames.shift()?.(0)
        expect(leftStrip.scrollLeft).toBeGreaterThan(0)
        expect(rightStrip.scrollLeft).toBe(200)
        fireEvent.pointerUp(window, { pointerId: 1, clientX: 295, clientY: 15 })
        raf.mockRestore()
    })
})

test("insertionSide splits an element at its midpoint", () => {
    const element = document.createElement("div")
    vi.spyOn(element, "getBoundingClientRect").mockReturnValue(rect(100, 0, 80, 20))
    expect(insertionSide(element, { x: 120, y: 0 }, "x")).toBe("before")
    expect(insertionSide(element, { x: 150, y: 0 }, "x")).toBe("after")
    expect(insertionSide(element, { x: 0, y: 5 }, "y")).toBe("before")
    expect(insertionSide(element, { x: 0, y: 15 }, "y")).toBe("after")
})

import { isMacPlatform } from "@/lib/platform"

/**
 * In-app drag and drop built on Pointer Events.
 *
 * Tauri's OS file-drop layer (dragDropEnabled, which Finder/Explorer path
 * drops need) answers every native drag session itself, so HTML5
 * `dragover`/`drop` never reach the page on macOS or Windows and jsdom tests
 * of them are false greens. Every in-app drag therefore tracks the pointer and
 * hit-tests with `elementFromPoint` through this module instead.
 *
 * DOM contract:
 * - Mark handles with `data-pointer-drag-handle` so WebKit never starts a
 *   native drag (which suppresses pointer events) and touch input stays ours.
 * - The handle gets `data-pointer-drag-source` while its drag is active.
 * - The resolved target gets `data-pointer-drop-target="before|after|inside"`.
 * - `<html>` gets `data-pointer-dragging` while any drag is active.
 */

export interface DragPoint {
    x: number
    y: number
}

export type PointerDropPosition = "before" | "after" | "inside"

export interface PointerDropTarget<T> {
    /** Element that shows the drop indicator. */
    element: HTMLElement
    /** Indicator written to `data-pointer-drop-target`; defaults to "inside". */
    position?: PointerDropPosition
    data: T
}

export interface PointerDragOptions<T> {
    /** Text that follows the cursor while dragging. */
    label?: string
    /** Logical px the press must travel before it becomes a drag. */
    threshold?: number
    /** Scroll containers that scroll while the pointer nears their edges. */
    autoScroll?: () => ReadonlyArray<Element | null | undefined>
    /** Drop target under the pointer, or null where nothing accepts the drop. */
    resolveTarget: (point: DragPoint) => PointerDropTarget<T> | null
    /** The press became a drag. */
    onStart?: () => void
    /** Released over a target. Runs after the drag state is cleaned up. */
    onDrop: (target: PointerDropTarget<T>, point: DragPoint) => void
    /** The gesture ended, dropped or not, once it had become a drag. */
    onEnd?: () => void
}

/** The fields of a pointerdown this module reads (React or DOM event). */
export interface PointerPress {
    button: number
    pointerId?: number
    clientX: number
    clientY: number
    ctrlKey?: boolean
    currentTarget: EventTarget | null
}

export const DEFAULT_DRAG_THRESHOLD = 4
const EDGE_ZONE = 28
const MAX_SCROLL_STEP = 16

export const DROP_TARGET_ATTRIBUTE = "data-pointer-drop-target"
const SOURCE_ATTRIBUTE = "data-pointer-drag-source"
const ROOT_ATTRIBUTE = "data-pointer-dragging"

let current: { cancel: () => void } | null = null

/** Topmost element at a viewport point; null where jsdom has no layout. */
export function elementAtPoint(point: DragPoint): Element | null {
    return document.elementFromPoint?.(point.x, point.y) ?? null
}

/** Before/after insertion by the midpoint of `element` along an axis. */
export function insertionSide(element: Element, point: DragPoint, axis: "x" | "y"): "before" | "after" {
    const rect = element.getBoundingClientRect()
    return axis === "x"
        ? point.x > rect.left + rect.width / 2 ? "after" : "before"
        : point.y > rect.top + rect.height / 2 ? "after" : "before"
}

/** Whether a press is being tracked or dragged anywhere in the app. */
export function isPointerDragActive(): boolean {
    return current !== null
}

/**
 * Tracks a primary-button press as a potential drag. Below the threshold the
 * press stays an ordinary click; past it the drag owns the gesture and the
 * click that the release synthesizes is swallowed. Returns a disposer for
 * unmount cleanup, or null when the press was not tracked.
 */
export function beginPointerDrag<T>(press: PointerPress, options: PointerDragOptions<T>): (() => void) | null {
    if (press.button !== 0) return null
    // macOS turns Control-click into a context click; it must never drag.
    if (press.ctrlKey && isMacPlatform()) return null
    // A new press means any earlier gesture already lost its release.
    current?.cancel()

    const source = press.currentTarget instanceof HTMLElement ? press.currentTarget : null
    const pointerId = press.pointerId
    const start: DragPoint = { x: press.clientX, y: press.clientY }
    const threshold = options.threshold ?? DEFAULT_DRAG_THRESHOLD
    let point = start
    let active = false
    let ended = false
    let target: PointerDropTarget<T> | null = null
    let preview: HTMLElement | null = null
    let frame = 0

    const matches = (event: Event) => (event as PointerEvent).pointerId === pointerId

    const showTarget = (next: PointerDropTarget<T> | null) => {
        const position = next?.position ?? "inside"
        if (target?.element !== next?.element) target?.element.removeAttribute(DROP_TARGET_ATTRIBUTE)
        if (next && next.element.getAttribute(DROP_TARGET_ATTRIBUTE) !== position) {
            next.element.setAttribute(DROP_TARGET_ATTRIBUTE, position)
        }
        target = next
    }

    const placePreview = () => {
        if (preview) preview.style.transform = `translate(${point.x + 14}px, ${point.y + 16}px)`
    }

    const autoScroll = () => {
        frame = 0
        if (!active || ended) return
        let scrolled = false
        for (const element of options.autoScroll?.() ?? []) {
            if (!element) continue
            const rect = element.getBoundingClientRect()
            const dx = element.scrollWidth > element.clientWidth && within(point.y, rect.top, rect.bottom)
                ? edgeStep(point.x, rect.left, rect.right) : 0
            const dy = element.scrollHeight > element.clientHeight && within(point.x, rect.left, rect.right)
                ? edgeStep(point.y, rect.top, rect.bottom) : 0
            if (!dx && !dy) continue
            const before = [element.scrollLeft, element.scrollTop]
            element.scrollLeft += dx
            element.scrollTop += dy
            if (element.scrollLeft !== before[0] || element.scrollTop !== before[1]) scrolled = true
        }
        if (!scrolled) return
        // Targets moved under a still pointer.
        showTarget(options.resolveTarget(point))
        frame = requestAnimationFrame(autoScroll)
    }

    const activate = () => {
        active = true
        document.documentElement.setAttribute(ROOT_ATTRIBUTE, "true")
        source?.setAttribute(SOURCE_ATTRIBUTE, "true")
        if (options.label) {
            preview = document.createElement("div")
            preview.className = "pointer-drag-preview"
            preview.setAttribute("aria-hidden", "true")
            preview.textContent = options.label
            document.body.appendChild(preview)
        }
        // Capture keeps the release coming when it happens outside the window.
        // It is an optimization: some WebViews refuse it once the native
        // pointer has crossed surfaces, and the window listeners still work.
        try {
            if (pointerId !== undefined) source?.setPointerCapture?.(pointerId)
        } catch {
            // Window listeners retain the gesture.
        }
        options.onStart?.()
    }

    const end = () => {
        if (ended) return
        ended = true
        window.removeEventListener("pointermove", onMove, true)
        window.removeEventListener("pointerup", onUp, true)
        window.removeEventListener("pointercancel", onCancel, true)
        window.removeEventListener("pointerdown", onStalePress, true)
        window.removeEventListener("keydown", onKey, true)
        window.removeEventListener("blur", onBlur)
        if (frame) cancelAnimationFrame(frame)
        frame = 0
        if (current === gesture) current = null
        if (!active) return
        showTarget(null)
        preview?.remove()
        preview = null
        source?.removeAttribute(SOURCE_ATTRIBUTE)
        document.documentElement.removeAttribute(ROOT_ATTRIBUTE)
        try {
            if (pointerId !== undefined && source?.hasPointerCapture?.(pointerId)) {
                source.releasePointerCapture(pointerId)
            }
        } catch {
            // Capture already ended with the pointer.
        }
        options.onEnd?.()
    }

    function onMove(event: PointerEvent) {
        if (!matches(event)) return
        point = { x: event.clientX, y: event.clientY }
        if ((event.buttons & 1) === 0) {
            // The release happened where no pointerup could reach us, before
            // or after the threshold.
            end()
            return
        }
        if (!active) {
            if (Math.hypot(point.x - start.x, point.y - start.y) < threshold) return
            activate()
        }
        event.preventDefault()
        placePreview()
        showTarget(options.resolveTarget(point))
        if (!frame && options.autoScroll) frame = requestAnimationFrame(autoScroll)
    }

    function onUp(event: PointerEvent) {
        if (!matches(event)) return
        if (!active) {
            end()
            return
        }
        point = { x: event.clientX, y: event.clientY }
        const dropTarget = options.resolveTarget(point)
        end()
        swallowNextClick()
        if (dropTarget) options.onDrop(dropTarget, point)
    }

    function onCancel(event: PointerEvent) {
        if (matches(event)) end()
    }

    function onStalePress() {
        end()
    }

    function onKey(event: KeyboardEvent) {
        if (event.key !== "Escape" || !active) return
        // Escape belongs to the drag, not to a dialog or terminal under it.
        event.preventDefault()
        event.stopPropagation()
        end()
        swallowClickOnRelease(pointerId)
    }

    function onBlur() {
        end()
    }

    const gesture = { cancel: end }
    current = gesture
    window.addEventListener("pointermove", onMove, true)
    window.addEventListener("pointerup", onUp, true)
    window.addEventListener("pointercancel", onCancel, true)
    window.addEventListener("pointerdown", onStalePress, true)
    window.addEventListener("keydown", onKey, true)
    window.addEventListener("blur", onBlur)
    return end
}

function within(value: number, low: number, high: number): boolean {
    return value >= low && value <= high
}

/** Scroll speed that grows as the pointer goes deeper into an edge zone. */
function edgeStep(value: number, low: number, high: number): number {
    if (high - low <= EDGE_ZONE * 2) return 0
    if (value < low + EDGE_ZONE) return -Math.ceil(MAX_SCROLL_STEP * Math.min(1, (low + EDGE_ZONE - value) / EDGE_ZONE))
    if (value > high - EDGE_ZONE) return Math.ceil(MAX_SCROLL_STEP * Math.min(1, (value - high + EDGE_ZONE) / EDGE_ZONE))
    return 0
}

function swallowClick(event: MouseEvent) {
    event.preventDefault()
    event.stopPropagation()
}

/**
 * The release of a drag synthesizes a click on the source or a common
 * ancestor. It is dispatched in the same task as pointerup, so dropping the
 * swallower on the next task leaves later, real clicks alone.
 */
function swallowNextClick() {
    window.addEventListener("click", swallowClick, true)
    window.setTimeout(() => window.removeEventListener("click", swallowClick, true), 0)
}

/** After Escape the button is still down; its eventual release is no click. */
function swallowClickOnRelease(pointerId: number | undefined) {
    const onRelease = (event: PointerEvent) => {
        if (event.pointerId !== pointerId) return
        cleanup()
        swallowNextClick()
    }
    const cleanup = () => {
        window.removeEventListener("pointerup", onRelease, true)
        window.removeEventListener("pointerdown", cleanup, true)
        window.removeEventListener("blur", cleanup)
    }
    window.addEventListener("pointerup", onRelease, true)
    window.addEventListener("pointerdown", cleanup, true)
    window.addEventListener("blur", cleanup)
}

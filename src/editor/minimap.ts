import { Compartment, type Extension } from "@codemirror/state"
import { EditorView, ViewPlugin, type PluginValue, type ViewUpdate } from "@codemirror/view"
import i18n from "@/lib/i18n"

// Reconfigured live by EditorPane so the Settings toggle takes effect on an
// already-open editor without rebuilding the view.
export const minimapCompartment = new Compartment()

// Above this line count the strip samples every Nth line so the bar DOM stays
// bounded (and building it never blocks typing on a huge file).
const MAX_BARS = 2000

// Bar geometry copied from the design reference (dc.html:3899-3903): leading
// whitespace drives the left inset, the remaining length drives the width, both
// clamped so long/deeply-indented lines don't overflow the strip.
export function minimapBarGeometry(text: string): { marginLeft: number; width: number } {
    const len = text.length
    const lead = len - text.replace(/^\s+/, "").length
    return {
        marginLeft: Math.min(40, lead * 1.6),
        width: Math.min(58, Math.max(3, (len - lead) * 1.1))
    }
}

export function minimapViewportGeometry(scrollTop: number, scrollHeight: number, viewportHeight: number, height: number) {
    const trackHeight = Math.max(0, height)
    const contentHeight = Math.max(0, scrollHeight)
    const visibleHeight = Math.max(0, viewportHeight)
    const maxScrollTop = Math.max(0, contentHeight - visibleHeight)
    const top = Math.min(maxScrollTop, Math.max(0, scrollTop))
    return {
        top: contentHeight > 0 ? top / contentHeight * trackHeight : 0,
        height: contentHeight > 0 ? Math.min(1, visibleHeight / contentHeight) * trackHeight : trackHeight,
        maxScrollTop,
    }
}

let minimapId = 0

class MinimapView implements PluginValue {
    private readonly panel: HTMLDivElement
    private readonly bars: HTMLDivElement
    private readonly viewport: HTMLDivElement
    private readonly resizeObserver: ResizeObserver
    private frame = 0
    private docDirty = true
    private geometryDirty = true
    private barPositions: Array<{ element: HTMLDivElement; from: number }> = []
    private drag: { pointerId: number; grabOffset: number } | null = null

    constructor(private readonly view: EditorView) {
        this.panel = document.createElement("div")
        this.panel.className = "yz-minimap"
        this.panel.tabIndex = 0
        this.panel.setAttribute("role", "scrollbar")
        this.panel.setAttribute("aria-orientation", "vertical")
        this.panel.setAttribute("aria-valuemin", "0")
        if (!view.scrollDOM.id) view.scrollDOM.id = `yz-editor-scroll-${++minimapId}`
        this.panel.setAttribute("aria-controls", view.scrollDOM.id)
        this.bars = document.createElement("div")
        this.bars.className = "yz-minimap-lines"
        this.bars.setAttribute("aria-hidden", "true")
        this.viewport = document.createElement("div")
        this.viewport.className = "yz-minimap-viewport"
        this.viewport.setAttribute("aria-hidden", "true")
        this.panel.append(this.bars, this.viewport)
        view.dom.appendChild(this.panel)
        this.panel.addEventListener("pointerdown", this.onPointerDown)
        this.panel.addEventListener("pointermove", this.onPointerMove)
        this.panel.addEventListener("pointerup", this.onPointerEnd)
        this.panel.addEventListener("pointercancel", this.onPointerEnd)
        this.panel.addEventListener("lostpointercapture", this.onPointerEnd)
        this.panel.addEventListener("keydown", this.onKeyDown)
        view.scrollDOM.addEventListener("scroll", this.schedule, { passive: true })
        i18n.on("languageChanged", this.schedule)
        this.resizeObserver = new ResizeObserver(() => { this.geometryDirty = true; this.schedule() })
        this.resizeObserver.observe(view.scrollDOM)
        this.resizeObserver.observe(view.contentDOM)
        this.resizeObserver.observe(this.panel)
        this.render()
        this.schedule()
    }

    update(update: ViewUpdate) {
        if (update.docChanged) this.docDirty = true
        if (update.docChanged || update.geometryChanged) this.geometryDirty = true
        if (update.docChanged || update.geometryChanged || update.viewportChanged) this.schedule()
    }

    private schedule = () => {
        if (this.frame) return
        this.frame = requestAnimationFrame(() => {
            this.frame = 0
            this.render()
        })
    }

    private geometry() {
        const scroll = this.view.scrollDOM
        return minimapViewportGeometry(scroll.scrollTop, scroll.scrollHeight, scroll.clientHeight, this.panel.clientHeight)
    }

    private syncViewport() {
        // Stay alongside the text viewport, including when a CodeMirror search
        // panel takes space above/below it. Never cover that panel's controls.
        this.panel.style.top = `${this.view.scrollDOM.offsetTop}px`
        this.panel.style.height = `${this.view.scrollDOM.offsetHeight}px`
        const geometry = this.geometry()
        this.panel.setAttribute("aria-label", i18n.t("label", { ns: "editorMinimap" }))
        this.panel.title = i18n.t("hint", { ns: "editorMinimap" })
        this.viewport.style.top = `${geometry.top}px`
        this.viewport.style.height = `${geometry.height}px`
        this.panel.setAttribute("aria-valuemax", String(geometry.maxScrollTop))
        this.panel.setAttribute("aria-valuenow", String(Math.round(Math.max(0, Math.min(geometry.maxScrollTop, this.view.scrollDOM.scrollTop)))))
    }

    private scrollToPointer(clientY: number, grabOffset: number) {
        const scroll = this.view.scrollDOM
        const height = this.panel.clientHeight
        if (height <= 0) return
        const y = clientY - this.panel.getBoundingClientRect().top - grabOffset
        scroll.scrollTop = Math.max(0, Math.min(this.geometry().maxScrollTop, y / height * scroll.scrollHeight))
        this.syncViewport()
    }

    private onPointerDown = (event: PointerEvent) => {
        if (event.button !== 0 || this.drag) return
        event.preventDefault()
        event.stopPropagation()
        const geometry = this.geometry()
        const y = event.clientY - this.panel.getBoundingClientRect().top
        const inViewport = y >= geometry.top && y <= geometry.top + geometry.height
        this.drag = { pointerId: event.pointerId, grabOffset: inViewport ? y - geometry.top : geometry.height / 2 }
        this.panel.setPointerCapture(event.pointerId)
        this.panel.focus({ preventScroll: true })
        this.scrollToPointer(event.clientY, this.drag.grabOffset)
    }

    private onPointerMove = (event: PointerEvent) => {
        if (this.drag?.pointerId !== event.pointerId) return
        event.preventDefault()
        event.stopPropagation()
        this.scrollToPointer(event.clientY, this.drag.grabOffset)
    }

    private onPointerEnd = (event: PointerEvent) => {
        if (this.drag?.pointerId !== event.pointerId) return
        this.drag = null
        if (this.panel.hasPointerCapture(event.pointerId)) this.panel.releasePointerCapture(event.pointerId)
    }

    private onKeyDown = (event: KeyboardEvent) => {
        const scroll = this.view.scrollDOM
        let top = scroll.scrollTop
        if (event.key === "ArrowDown") top += this.view.defaultLineHeight
        else if (event.key === "ArrowUp") top -= this.view.defaultLineHeight
        else if (event.key === "PageDown") top += scroll.clientHeight
        else if (event.key === "PageUp") top -= scroll.clientHeight
        else if (event.key === "Home") top = 0
        else if (event.key === "End") top = this.geometry().maxScrollTop
        else return
        event.preventDefault()
        event.stopPropagation()
        scroll.scrollTop = Math.max(0, Math.min(this.geometry().maxScrollTop, top))
        this.syncViewport()
    }

    private render() {
        if (this.docDirty) {
            this.docDirty = false
            const { doc } = this.view.state
            const total = doc.lines
            const step = total > MAX_BARS ? Math.ceil(total / MAX_BARS) : 1
            const frag = document.createDocumentFragment()
            const count = Math.ceil(total / step)
            while (this.barPositions.length > count) this.barPositions.pop()!.element.remove()
            this.bars.style.setProperty("--yz-minimap-bar-count", String(count))
            let index = 0
            for (let n = 1; n <= total; n += step) {
                const line = doc.line(n)
                // The strip is a sample: don't scan a multi-megabyte minified
                // line or replace up to 2000 DOM nodes on every keystroke.
                const { marginLeft, width } = minimapBarGeometry(line.text.slice(0, 256))
                let position = this.barPositions[index++]
                if (!position) {
                    const element = document.createElement("div")
                    element.className = "yz-minimap-bar"
                    position = { element, from: line.from }
                    this.barPositions.push(position)
                    frag.appendChild(element)
                }
                position.from = line.from
                const { element } = position
                if (element.style.marginLeft !== `${marginLeft}%`) element.style.marginLeft = `${marginLeft}%`
                if (element.style.width !== `${width}%`) element.style.width = `${width}%`
            }
            this.bars.appendChild(frag)
        }
        if (this.geometryDirty) {
            this.geometryDirty = false
            const height = this.view.scrollDOM.scrollHeight || this.view.contentHeight
            let previousBlock = -1
            for (const { element, from } of this.barPositions) {
                const block = this.view.lineBlockAt(from)
                element.hidden = block.from === previousBlock
                previousBlock = block.from
                element.style.top = `${height > 0 ? (block.top + this.view.documentPadding.top) / height * 100 : 0}%`
            }
        }
        this.syncViewport()
    }

    destroy() {
        if (this.frame) cancelAnimationFrame(this.frame)
        this.resizeObserver.disconnect()
        this.view.scrollDOM.removeEventListener("scroll", this.schedule)
        i18n.off("languageChanged", this.schedule)
        if (this.drag && this.panel.hasPointerCapture(this.drag.pointerId)) this.panel.releasePointerCapture(this.drag.pointerId)
        this.drag = null
        this.panel.removeEventListener("pointerdown", this.onPointerDown)
        this.panel.removeEventListener("pointermove", this.onPointerMove)
        this.panel.removeEventListener("pointerup", this.onPointerEnd)
        this.panel.removeEventListener("pointercancel", this.onPointerEnd)
        this.panel.removeEventListener("lostpointercapture", this.onPointerEnd)
        this.panel.removeEventListener("keydown", this.onKeyDown)
        this.panel.remove()
    }
}

const minimapPlugin = ViewPlugin.fromClass(MinimapView)

// The extension value for the compartment: the plugin when on, nothing when off.
export function minimap(enabled: boolean): Extension {
    return enabled ? minimapPlugin : []
}

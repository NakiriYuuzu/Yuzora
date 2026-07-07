import { Compartment, type Extension } from "@codemirror/state"
import { EditorView, ViewPlugin, type PluginValue, type ViewUpdate } from "@codemirror/view"

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

class MinimapView implements PluginValue {
    private readonly panel: HTMLDivElement
    private frame = 0

    constructor(private readonly view: EditorView) {
        this.panel = document.createElement("div")
        this.panel.className = "yz-minimap"
        this.panel.setAttribute("aria-hidden", "true")
        view.dom.appendChild(this.panel)
        this.render()
    }

    update(update: ViewUpdate) {
        // Line-density only depends on the text; skip selection/viewport churn.
        if (update.docChanged) this.schedule()
    }

    // Coalesce bursts of edits into one rebuild per frame.
    private schedule() {
        if (this.frame) return
        this.frame = requestAnimationFrame(() => {
            this.frame = 0
            this.render()
        })
    }

    private render() {
        const { doc } = this.view.state
        const total = doc.lines
        const step = total > MAX_BARS ? Math.ceil(total / MAX_BARS) : 1
        const frag = document.createDocumentFragment()
        for (let n = 1; n <= total; n += step) {
            const { marginLeft, width } = minimapBarGeometry(doc.line(n).text)
            const bar = document.createElement("div")
            bar.className = "yz-minimap-bar"
            bar.style.marginLeft = `${marginLeft}%`
            bar.style.width = `${width}%`
            frag.appendChild(bar)
        }
        this.panel.replaceChildren(frag)
    }

    destroy() {
        if (this.frame) cancelAnimationFrame(this.frame)
        this.panel.remove()
    }
}

const minimapPlugin = ViewPlugin.fromClass(MinimapView)

// The extension value for the compartment: the plugin when on, nothing when off.
export function minimap(enabled: boolean): Extension {
    return enabled ? minimapPlugin : []
}

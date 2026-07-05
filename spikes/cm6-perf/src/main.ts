import { EditorState, type Extension } from "@codemirror/state"
import { EditorView, keymap } from "@codemirror/view"
import { defaultKeymap } from "@codemirror/commands"
import { javascript } from "@codemirror/lang-javascript"
import { syntaxHighlighting, defaultHighlightStyle } from "@codemirror/language"

interface Cm6PerfMetrics {
    fixture: string
    syntaxOff: boolean
    chars: number
    lines: number
    fetchMs: number
    stateCreateMs: number
    viewCreateMs: number
    firstPaintMs: number
}

interface Cm6PerfApi {
    ready: boolean
    error: string | null
    metrics: Cm6PerfMetrics | null
    longTasks: { start: number; duration: number }[]
    keyLatencies: number[]
    focusMiddle: () => void
    clearKeyLatencies: () => void
    scrollThrough: (
        steps: number,
        stepPx: number,
    ) => Promise<{ ms: number; longTaskCountDuring: number; longTaskDurationDuring: number }>
}

declare global {
    interface Window {
        cm6Perf: Cm6PerfApi
    }
}

const longTasks: { start: number; duration: number }[] = []
try {
    new PerformanceObserver((list) => {
        for (const entry of list.getEntries()) {
            longTasks.push({ start: entry.startTime, duration: entry.duration })
        }
    }).observe({ entryTypes: ["longtask"] })
} catch {
    // longtask entry type unsupported in this browser
}

const keyLatencies: number[] = []
let lastKeyTime = 0
window.addEventListener(
    "keydown",
    () => {
        lastKeyTime = performance.now()
    },
    true,
)

const api: Cm6PerfApi = {
    ready: false,
    error: null,
    metrics: null,
    longTasks,
    keyLatencies,
    focusMiddle: () => {},
    clearKeyLatencies: () => {
        keyLatencies.length = 0
    },
    scrollThrough: async () => ({ ms: 0, longTaskCountDuring: 0, longTaskDurationDuring: 0 }),
}
window.cm6Perf = api

async function main() {
    const params = new URLSearchParams(location.search)
    const fixture = params.get("fixture") ?? "f-1mb.ts"
    const syntaxOff = params.get("syntaxOff") === "1"

    const t0 = performance.now()
    const res = await fetch(`/fixture/${encodeURIComponent(fixture)}`)
    if (!res.ok) throw new Error(`fixture fetch failed: ${res.status} ${res.statusText}`)
    const text = await res.text()
    const fetchMs = performance.now() - t0

    const updateListener = EditorView.updateListener.of((update) => {
        if (update.docChanged && lastKeyTime) {
            const kt = lastKeyTime
            lastKeyTime = 0
            requestAnimationFrame(() => {
                keyLatencies.push(performance.now() - kt)
            })
        }
    })

    const extensions: Extension[] = [keymap.of(defaultKeymap), updateListener]
    if (!syntaxOff) {
        extensions.push(javascript())
        extensions.push(syntaxHighlighting(defaultHighlightStyle))
    }

    const sc0 = performance.now()
    const state = EditorState.create({ doc: text, extensions })
    const stateCreateMs = performance.now() - sc0

    const vc0 = performance.now()
    const view = new EditorView({ state, parent: document.getElementById("editor")! })
    const viewCreateMs = performance.now() - vc0

    const firstPaintMs = await new Promise<number>((resolvePaint) => {
        const paintStart = performance.now()
        requestAnimationFrame(() => {
            requestAnimationFrame(() => resolvePaint(performance.now() - paintStart))
        })
    })

    api.metrics = {
        fixture,
        syntaxOff,
        chars: text.length,
        lines: state.doc.lines,
        fetchMs,
        stateCreateMs,
        viewCreateMs,
        firstPaintMs,
    }

    api.focusMiddle = () => {
        const mid = Math.floor(state.doc.length / 2)
        view.dispatch({
            selection: { anchor: mid },
            effects: EditorView.scrollIntoView(mid, { y: "center" }),
        })
        view.focus()
    }

    api.scrollThrough = async (steps = 40, stepPx = 1200) => {
        const scroller = view.scrollDOM
        const before = longTasks.length
        const beforeDur = longTasks.reduce((sum, t) => sum + t.duration, 0)
        const start = performance.now()
        for (let i = 0; i < steps; i++) {
            scroller.scrollTop = Math.min(scroller.scrollTop + stepPx, scroller.scrollHeight)
            scroller.scrollLeft = Math.min(scroller.scrollLeft + stepPx, scroller.scrollWidth)
            await new Promise((r) => requestAnimationFrame(r))
        }
        const ms = performance.now() - start
        const after = longTasks.length
        const afterDur = longTasks.reduce((sum, t) => sum + t.duration, 0)
        return {
            ms,
            longTaskCountDuring: after - before,
            longTaskDurationDuring: afterDur - beforeDur,
        }
    }

    api.ready = true
}

main().catch((err) => {
    api.error = err instanceof Error ? `${err.message}\n${err.stack ?? ""}` : String(err)
    api.ready = true
    console.error(err)
})

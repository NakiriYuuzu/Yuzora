import { EditorSelection, Prec, type Text } from "@codemirror/state"
import { EditorView, ViewPlugin, keymap, type ViewUpdate } from "@codemirror/view"
import { findNext, findPrevious, getSearchQuery, openSearchPanel, searchPanelOpen } from "@codemirror/search"
import i18n from "@/lib/i18n"
import type { SearchAction, SearchTask, SearchTaskResult } from "./searchTask"

// Measured in UTF-16 code units (CodeMirror doc.length): search cost scales with
// characters scanned, not with the file's encoded byte size.
const LARGE_DOCUMENT_CHARS = 256 * 1024
const ACTIONS = new Set<SearchAction>(["next", "prev", "select", "replace", "replaceAll"])

export function largeFileSearch(createWorker = () => new Worker(new URL("./search.worker.ts", import.meta.url), { type: "module" })) {
    class BackgroundSearch {
        private worker: Worker | null = null
        private cachedDoc: Text | null = null
        private sequence = 0
        private pending = false
        private timer: ReturnType<typeof setTimeout> | undefined

        constructor(private view: EditorView) {
            // CodeMirror exposes commands but no async hook for its built-in
            // panel buttons. Keep its panel/keyboard behavior and intercept only
            // full-document operations; visible-range highlighting stays local.
            view.dom.addEventListener("click", this.click, true)
            view.dom.addEventListener("keydown", this.keydown, true)
        }

        private status(key: string | null) {
            const panel = this.view.dom.querySelector<HTMLElement>(".cm-search")
            if (!panel) return
            panel.setAttribute("aria-busy", String(this.pending))
            let status = panel.querySelector<HTMLElement>(".yz-search-status")
            if (!status && key) {
                status = document.createElement("span")
                status.className = "yz-search-status"
                status.setAttribute("role", "status")
                panel.append(status)
            }
            if (status) status.textContent = key ? i18n.t(key, { ns: "editorSearch" }) : ""
        }

        private click = (event: MouseEvent) => {
            const button = event.target instanceof Element ? event.target.closest<HTMLButtonElement>(".cm-search button[name]") : null
            if (button && ACTIONS.has(button.name as SearchAction) && this.run(button.name as SearchAction)) {
                event.preventDefault()
                event.stopImmediatePropagation()
            }
        }

        private keydown = (event: KeyboardEvent) => {
            if (event.isComposing || event.key !== "Enter" || !(event.target instanceof HTMLInputElement) || !event.target.closest(".cm-search")) return
            const name = event.target.name
            if (name !== "search" && name !== "replace") return
            // The stock panel commits on keyup/change, so commit the latest input
            // before Enter (including paste/IME input) starts the worker.
            event.target.dispatchEvent(new Event("change", { bubbles: true }))
            if (this.run(name === "replace" ? "replace" : event.shiftKey ? "prev" : "next")) {
                event.preventDefault()
                event.stopImmediatePropagation()
            }
        }

        run(action: SearchAction): boolean {
            const { state } = this.view
            if (state.doc.length < LARGE_DOCUMENT_CHARS) return false
            const query = getSearchQuery(state)
            if (!query.valid) { openSearchPanel(this.view); return true }
            if (state.readOnly && (action === "replace" || action === "replaceAll")) return true
            if (this.pending) this.release()
            const id = ++this.sequence
            this.pending = true
            this.status("searching")
            try {
                this.worker ??= createWorker()
                this.worker.onmessage = (event: MessageEvent<SearchTaskResult>) => {
                    const result = event.data
                    if (id !== this.sequence || result.id !== id) return
                    this.pending = false
                    clearTimeout(this.timer)
                    if (this.view.state.doc !== state.doc || !this.view.state.selection.eq(state.selection) || !getSearchQuery(this.view.state).eq(query)) return
                    this.status(result.error ?? (result.ranges.length || result.changes?.length ? null : "noMatch"))
                    if (result.error) return
                    const changes = this.view.state.changes(result.changes ?? [])
                    const selection = result.ranges.length ? EditorSelection.create(result.ranges.map(range => EditorSelection.range(range.from, range.to))).map(changes) : undefined
                    if (!changes.empty || selection) this.view.dispatch({
                        changes, selection,
                        effects: selection ? EditorView.scrollIntoView(selection.main) : [],
                        userEvent: changes.empty ? "select.search" : "input.replace",
                    })
                }
                this.worker.onerror = () => { this.release(); this.status("failed") }
                const task: SearchTask = {
                    id, action, from: state.selection.main.from, to: state.selection.main.to,
                    wordChars: state.languageDataAt<string>("wordChars", state.selection.main.head).join(""),
                    query: { search: query.search, replace: query.replace, caseSensitive: query.caseSensitive, regexp: query.regexp, wholeWord: query.wholeWord, literal: query.literal },
                    ...(this.cachedDoc === state.doc ? {} : { lines: state.doc.toJSON() }),
                }
                this.worker.postMessage(task)
                this.cachedDoc = state.doc
                this.timer = setTimeout(() => { this.release(); this.status("timeout") }, 5000)
            } catch { this.release(); this.status("failed") }
            return true
        }

        update(update: ViewUpdate) {
            if (update.docChanged || !searchPanelOpen(update.state) || (this.pending && (update.selectionSet || !getSearchQuery(update.startState).eq(getSearchQuery(update.state))))) {
                this.release()
                this.status(null)
            }
        }

        private release() {
            ++this.sequence
            this.pending = false
            clearTimeout(this.timer)
            this.worker?.terminate()
            this.worker = null
            this.cachedDoc = null
        }

        destroy() {
            this.release()
            this.view.dom.removeEventListener("click", this.click, true)
            this.view.dom.removeEventListener("keydown", this.keydown, true)
        }
    }
    const plugin = ViewPlugin.fromClass(BackgroundSearch)
    const next = (view: EditorView) => view.plugin(plugin)?.run("next") || findNext(view)
    const previous = (view: EditorView) => view.plugin(plugin)?.run("prev") || findPrevious(view)
    return [plugin, Prec.highest(keymap.of([
        { key: "F3", run: next, shift: previous, scope: "editor search-panel", preventDefault: true },
        { key: "Mod-g", run: next, shift: previous, scope: "editor search-panel", preventDefault: true },
    ]))]
}

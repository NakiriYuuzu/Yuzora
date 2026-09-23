import { afterEach, expect, it, vi } from "vitest"
import { EditorState } from "@codemirror/state"
import { EditorView } from "@codemirror/view"
import { closeSearchPanel, openSearchPanel, SearchQuery, setSearchQuery } from "@codemirror/search"
import { largeFileSearch } from "./largeFileSearch"
import { runSearchTask, type SearchTask, type SearchTaskResult } from "./searchTask"

let view: EditorView | undefined
afterEach(() => { view?.destroy(); view = undefined })

function setup() {
    const worker = { postMessage: vi.fn<(task: SearchTask) => void>(), terminate: vi.fn(), onmessage: null as null | ((event: MessageEvent<SearchTaskResult>) => void), onerror: null }
    view = new EditorView({ state: EditorState.create({ doc: "other text\n".repeat(40000) + "needle", extensions: largeFileSearch(() => worker as unknown as Worker) }), parent: document.body })
    openSearchPanel(view)
    view.dispatch({ effects: setSearchQuery.of(new SearchQuery({ search: "needle" })) })
    return worker
}

it("keeps the UI responsive until the worker returns and reuses an unchanged document", () => {
    const worker = setup()
    const doc = view!.state.doc
    const click = () => view!.dom.querySelector<HTMLButtonElement>('button[name="next"]')!.click()
    click()
    expect(view!.state.selection.main.to).toBe(0)
    expect(view!.dom.querySelector(".cm-search")).toHaveAttribute("aria-busy", "true")
    const request = worker.postMessage.mock.calls[0][0]
    expect(request.lines).toHaveLength(40001)
    worker.onmessage!({ data: runSearchTask(doc, request) } as MessageEvent<SearchTaskResult>)
    expect(view!.state.selection.main.to).toBe(doc.length)
    expect(view!.dom.querySelector(".cm-search")).toHaveAttribute("aria-busy", "false")
    click()
    expect(worker.postMessage.mock.calls[1][0].lines).toBeUndefined()
})

it.each(["query", "document", "close", "destroy"])("cancels work on %s changes and ignores a late response", reason => {
    const worker = setup()
    view!.dom.querySelector<HTMLButtonElement>('button[name="next"]')!.click()
    const deliver = worker.onmessage!
    const result = runSearchTask(view!.state.doc, worker.postMessage.mock.calls[0][0])
    if (reason === "query") view!.dispatch({ effects: setSearchQuery.of(new SearchQuery({ search: "different" })) })
    if (reason === "document") view!.dispatch({ changes: { from: 0, insert: "changed" } })
    if (reason === "close") closeSearchPanel(view!)
    if (reason === "destroy") { view!.destroy(); view = undefined }
    expect(worker.terminate).toHaveBeenCalledOnce()
    deliver({ data: result } as MessageEvent<SearchTaskResult>)
    if (view) expect(view.state.selection.main.to).toBeLessThan(10)
})

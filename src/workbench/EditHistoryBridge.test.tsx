import { cleanup, render } from "@testing-library/react"
import { afterEach, expect, it, vi } from "vitest"
import { history } from "@codemirror/commands"
import { EditorState } from "@codemirror/state"
import { EditorView } from "@codemirror/view"

const events = vi.hoisted(() => ({
    handler: null as ((event: { payload: "undo" | "redo" | "selectAll" }) => void) | null
}))
const ipc = vi.hoisted(() => ({ editHistoryNative: vi.fn(async (_action: string) => {}) }))

vi.mock("@tauri-apps/api/event", () => ({
    listen: (_event: string, handler: typeof events.handler) => {
        events.handler = handler
        return Promise.resolve(() => {
            events.handler = null
        })
    }
}))
vi.mock("@/lib/ipc", () => ({ editHistoryNative: ipc.editHistoryNative }))

import { runEditorHistory } from "@/editor/editHistory"
import { EditHistoryBridge } from "./EditHistoryBridge"

let view: EditorView | null = null

afterEach(() => {
    cleanup()
    vi.restoreAllMocks()
    view?.destroy()
    view = null
    document.body.innerHTML = ""
    ipc.editHistoryNative.mockClear()
})

function editedView(): EditorView {
    const parent = document.createElement("div")
    document.body.appendChild(parent)
    view = new EditorView({ state: EditorState.create({ doc: "alpha", extensions: [history()] }), parent })
    view.dispatch({ changes: { from: 5, insert: " beta" }, userEvent: "input.type" })
    return view
}

it("undoes and redoes in the focused CodeMirror editor", () => {
    const editor = editedView()
    expect(runEditorHistory("undo", editor.contentDOM)).toBe(true)
    expect(editor.state.doc.toString()).toBe("alpha")
    expect(runEditorHistory("redo", editor.contentDOM)).toBe(true)
    expect(editor.state.doc.toString()).toBe("alpha beta")
})

it("leaves inputs, editor panel fields and no focus to the native action", () => {
    const editor = editedView()
    const input = document.createElement("input")
    document.body.appendChild(input)
    const panelField = document.createElement("input")
    editor.dom.appendChild(panelField)
    expect(runEditorHistory("undo", input)).toBe(false)
    expect(runEditorHistory("undo", panelField)).toBe(false)
    expect(runEditorHistory("undo", null)).toBe(false)
    expect(editor.state.doc.toString()).toBe("alpha beta")
})

it("routes Edit menu events to the focused editor, otherwise to the native responder chain", () => {
    const editor = editedView()
    render(<EditHistoryBridge />)
    const input = document.createElement("input")
    document.body.appendChild(input)
    input.focus()
    events.handler!({ payload: "undo" })
    expect(ipc.editHistoryNative).toHaveBeenCalledWith("undo")
    expect(editor.state.doc.toString()).toBe("alpha beta")

    ipc.editHistoryNative.mockClear()
    vi.spyOn(document, "activeElement", "get").mockReturnValue(editor.contentDOM)
    events.handler!({ payload: "undo" })
    expect(ipc.editHistoryNative).not.toHaveBeenCalled()
    expect(editor.state.doc.toString()).toBe("alpha")
})

it("leaves the action native when a child Browser webview owns keyboard focus over a stale editor activeElement", () => {
    const editor = editedView()
    render(<EditHistoryBridge />)
    vi.spyOn(document, "activeElement", "get").mockReturnValue(editor.contentDOM)
    vi.spyOn(document, "hasFocus").mockReturnValue(false)
    events.handler!({ payload: "undo" })
    expect(ipc.editHistoryNative).toHaveBeenCalledWith("undo")
    expect(editor.state.doc.toString()).toBe("alpha beta")
})

it("drops a Select All that would only highlight the page behind a marked dialog", () => {
    render(<EditHistoryBridge />)
    const dialog = document.createElement("div")
    dialog.setAttribute("data-contain-select-all", "")
    const checkbox = document.createElement("button")
    checkbox.setAttribute("role", "checkbox")
    const value = document.createElement("textarea")
    dialog.append(checkbox, value)
    const outside = document.createElement("button")
    document.body.append(dialog, outside)
    vi.spyOn(document, "hasFocus").mockReturnValue(true)

    checkbox.focus()
    events.handler!({ payload: "selectAll" })
    expect(ipc.editHistoryNative).not.toHaveBeenCalled()

    value.focus()
    events.handler!({ payload: "selectAll" })
    expect(ipc.editHistoryNative).toHaveBeenLastCalledWith("selectAll")

    ipc.editHistoryNative.mockClear()
    outside.focus()
    events.handler!({ payload: "selectAll" })
    expect(ipc.editHistoryNative).toHaveBeenLastCalledWith("selectAll")
})

it("leaves Select All native when a child Browser webview owns keyboard focus", () => {
    render(<EditHistoryBridge />)
    const dialog = document.createElement("div")
    dialog.setAttribute("data-contain-select-all", "")
    const checkbox = document.createElement("button")
    dialog.append(checkbox)
    document.body.append(dialog)
    checkbox.focus()
    vi.spyOn(document, "hasFocus").mockReturnValue(false)
    events.handler!({ payload: "selectAll" })
    expect(ipc.editHistoryNative).toHaveBeenCalledWith("selectAll")
})

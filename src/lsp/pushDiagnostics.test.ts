import { afterEach, expect, it, vi } from "vitest"
import { ChangeSet, EditorState } from "@codemirror/state"
import { EditorView } from "@codemirror/view"
import { diagnosticCount, forEachDiagnostic } from "@codemirror/lint"
import { LSPPlugin, type LSPClient } from "@codemirror/lsp-client"
import { guardedServerDiagnostics } from "./pushDiagnostics"

const views: EditorView[] = []
afterEach(() => { views.splice(0).forEach((view) => view.destroy()); vi.restoreAllMocks() })

function fixture() {
    const view = new EditorView({ state: EditorState.create({ doc: "const x = 1;\n" }) })
    views.push(view)
    const doc = view.state.doc
    vi.spyOn(LSPPlugin, "get").mockReturnValue({
        syncedDoc: doc,
        unsyncedChanges: ChangeSet.empty(doc.length),
        fromPosition: (position: { line: number; character: number }) => doc.line(position.line + 1).from + position.character
    } as LSPPlugin)
    const file = { version: 3, getView: () => view }
    const client = { workspace: { getFile: (uri: string) => uri === "file:///fixture.ts" ? file : null } } as unknown as LSPClient
    const extension = guardedServerDiagnostics()
    const deliver = (diagnostics: unknown[], version?: number) => extension.notificationHandlers!["textDocument/publishDiagnostics"](
        client, { uri: "file:///fixture.ts", diagnostics, version }
    )
    return { view, deliver, extension }
}
const diagnostic = (line = 0, start = 6, end = 7) => ({
    range: { start: { line, character: start }, end: { line, character: end } }, message: "fixture diagnostic", severity: 1
})

it.each([
    diagnostic(1, 0, 8), diagnostic(4, 0, 1), diagnostic(0, -1, 3), diagnostic(0, 7, 6)
])("preserves valid diagnostics when a stale unversioned report has an invalid range: %j", (stale) => {
    const { view, deliver } = fixture()
    deliver([diagnostic()])
    expect(diagnosticCount(view.state)).toBe(1)
    expect(() => deliver([stale])).not.toThrow()
    expect(diagnosticCount(view.state)).toBe(1)
    deliver([])
    expect(diagnosticCount(view.state)).toBe(0)
})

it("retains the library's version rejection, mapping and autosync", () => {
    const { view, deliver, extension } = fixture()
    expect(extension.editorExtension).toBeDefined()
    deliver([diagnostic()], 2)
    expect(diagnosticCount(view.state)).toBe(0)
    const plugin = LSPPlugin.get(view)!
    view.dispatch({ changes: { from: 0, insert: "//\n" } })
    plugin.unsyncedChanges = ChangeSet.of({ from: 0, insert: "//\n" }, plugin.syncedDoc.length)
    deliver([diagnostic()], 3)
    const positions: number[] = []
    forEachDiagnostic(view.state, (_d, from, to) => positions.push(from, to))
    expect(positions).toEqual([9, 10])
})

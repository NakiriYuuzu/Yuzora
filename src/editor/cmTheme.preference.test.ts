import { afterEach, expect, it } from "vitest"
import { EditorView } from "@codemirror/view"
import { appTheme } from "./cmTheme"
import { useEditorSettingsStore } from "@/state/editorSettingsStore"

afterEach(() => useEditorSettingsStore.setState({ syntaxTheme: "github" }))
it("updates every mounted editor and diff view without rebuilding documents", () => {
    const views = [new EditorView({ doc: "editor", extensions: [appTheme] }), new EditorView({ doc: "diff", extensions: [appTheme] })]
    try {
        useEditorSettingsStore.getState().setSyntaxTheme("one")
        expect(views.map(view => view.dom.dataset.syntaxTheme)).toEqual(["one", "one"])
        expect(views.map(view => view.state.doc.toString())).toEqual(["editor", "diff"])
        views[0].destroy()
        useEditorSettingsStore.getState().setSyntaxTheme("yuzora")
        expect(views[1].dom.dataset.syntaxTheme).toBe("yuzora")
    } finally { views[1].destroy() }
})

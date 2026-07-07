import { expect, test } from "vitest"
import { EditorState } from "@codemirror/state"
import { EditorView } from "@codemirror/view"
import { tags } from "@lezer/highlight"
import { appHighlightStyle, appTheme } from "./cmTheme"

test("appHighlightStyle 對 keyword tag 產生 class", () => {
    expect(appHighlightStyle.style([tags.keyword])).toBeTruthy()
    expect(appHighlightStyle.style([tags.string])).toBeTruthy()
})

test("EditorView 套用 appTheme 後 .cm-editor 帶主題 scope class", () => {
    const view = new EditorView({
        state: EditorState.create({ doc: "x", extensions: [appTheme] }),
        parent: document.body
    })
    const classes = view.dom.className.split(/\s+/).filter(Boolean)
    // EditorView.theme() 會為 editor 注入一個 generated scope class，不只 cm-editor
    expect(classes.some((c) => c !== "cm-editor" && c !== "cm-focused")).toBe(true)
    view.destroy()
})

import { expect, test } from "vitest"
import { EditorState } from "@codemirror/state"
import { EditorView } from "@codemirror/view"
import { highlightingFor } from "@codemirror/language"
import { tags } from "@lezer/highlight"
import { Compartment } from "@codemirror/state"
import { buildExtensions, hasVeryLongLine, languageExtensionFromPath } from "./cmExtensions"
import { minimap, minimapBarGeometry, minimapCompartment } from "./minimap"
import { appTheme } from "./cmTheme"

test("hasVeryLongLine 偵測超長單行", () => {
    expect(hasVeryLongLine("short\nlines\n")).toBe(false)
    expect(hasVeryLongLine(`a\n${"x".repeat(10_001)}\nb`)).toBe(true)
})

test("languageExtensionFromPath 對已知副檔名回傳 extension、未知回傳 null", () => {
    expect(languageExtensionFromPath("/a.ts")).not.toBeNull()
    expect(languageExtensionFromPath("/a.rs")).not.toBeNull()
    expect(languageExtensionFromPath("/a.unknown")).toBeNull()
})

test("languageExtensionFromPath 覆蓋新增語言（官方套件與 legacy-modes）", () => {
    expect(languageExtensionFromPath("/a.yaml")).not.toBeNull()
    expect(languageExtensionFromPath("/a.go")).not.toBeNull()
    expect(languageExtensionFromPath("/a.sh")).not.toBeNull()
    expect(languageExtensionFromPath("/a.toml")).not.toBeNull()
    expect(languageExtensionFromPath("/path/to/Dockerfile")).not.toBeNull()
})

test("buildExtensions 掛上 syntaxHighlighting，語法節點會渲染成帶 class 的 span", () => {
    const extensions = buildExtensions("/a.ts", { readonly: false, syntaxOff: false }, () => {}, () => {}, false)
    const view = new EditorView({
        state: EditorState.create({ doc: 'const x = "hi"', extensions }),
        parent: document.body
    })
    expect(view.dom.querySelectorAll(".cm-line span[class]").length).toBeGreaterThan(0)
    view.destroy()
})

test("buildExtensions minimapEnabled 決定 .yz-minimap 是否掛出，且可經 compartment 即時開關", () => {
    const view = new EditorView({
        state: EditorState.create({
            doc: "one\ntwo\nthree",
            extensions: buildExtensions("/a.ts", { readonly: false, syntaxOff: false }, () => {}, () => {}, false)
        }),
        parent: document.body
    })
    // Built with minimap off — no strip.
    expect(view.dom.querySelector(".yz-minimap")).toBeNull()
    // Reconfigure the compartment on: strip appears with one bar per line.
    view.dispatch({ effects: minimapCompartment.reconfigure(minimap(true)) })
    const panel = view.dom.querySelector(".yz-minimap")
    expect(panel).not.toBeNull()
    expect(panel!.querySelectorAll(".yz-minimap-bar").length).toBe(3)
    // Reconfigure back off: strip is removed.
    view.dispatch({ effects: minimapCompartment.reconfigure(minimap(false)) })
    expect(view.dom.querySelector(".yz-minimap")).toBeNull()
    view.destroy()
})

test("buildExtensions minimapEnabled=true 直接建構就掛出 .yz-minimap", () => {
    const view = new EditorView({
        state: EditorState.create({
            doc: "a\nb",
            extensions: buildExtensions("/a.ts", { readonly: false, syntaxOff: false }, () => {}, () => {}, true)
        }),
        parent: document.body
    })
    expect(view.dom.querySelector(".yz-minimap")).not.toBeNull()
    view.destroy()
})

test("minimapCompartment 是 CodeMirror Compartment（供 EditorPane reconfigure）", () => {
    expect(minimapCompartment).toBeInstanceOf(Compartment)
})

test("minimapBarGeometry：縮排推 margin、長度推 width，皆有上下限", () => {
    // Empty line clamps to the minimum width, zero inset.
    expect(minimapBarGeometry("")).toEqual({ marginLeft: 0, width: 3 })
    // Indentation drives margin (4 spaces × 1.6 = 6.4), content length drives width.
    expect(minimapBarGeometry("    ab")).toEqual({ marginLeft: 6.4, width: 3 })
    // Both clamp: deep indent caps at 40%, long line caps at 58%.
    const long = minimapBarGeometry(" ".repeat(50) + "x".repeat(200))
    expect(long.marginLeft).toBe(40)
    expect(long.width).toBe(58)
})

test("buildExtensions 掛上 app 主題與語法高亮", () => {
    const extensions = buildExtensions("/a.ts", { readonly: false, syntaxOff: false }, () => {}, () => {}, false)
    // appTheme 以固定 identity 掛在產物頂層
    expect(extensions.includes(appTheme)).toBe(true)
    // appHighlightStyle 為非 fallback highlighter，keyword tag 會拿到 class
    const state = EditorState.create({ doc: "const x = 1", extensions })
    expect(highlightingFor(state, [tags.keyword])).toBeTruthy()
})

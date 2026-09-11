import { afterEach, describe, expect, it } from "vitest"
import { EditorState } from "@codemirror/state"
import { ensureSyntaxTree } from "@codemirror/language"
import { javascript } from "@codemirror/lang-javascript"
import { highlightTree, tags } from "@lezer/highlight"
import { appHighlightStyle } from "./cmTheme"
// @ts-expect-error Node types are excluded from the browser tsconfig; Vitest runs this test in Node.
import { readFileSync } from "node:fs"
const paletteCss = readFileSync("src/editor/editor.css", "utf8")

afterEach(() => { document.documentElement.classList.remove("dark"); document.body.replaceChildren(); document.querySelector('[data-theme-test]')?.remove() })

describe("function role specificity", () => {
    it("maps all function tag variants to the function role without recoloring ordinary definitions", () => {
        const functionClass = appHighlightStyle.style([tags.function(tags.variableName)])
        for (const tag of [tags.function(tags.definition(tags.variableName)), tags.function(tags.propertyName), tags.function(tags.definition(tags.propertyName))]) {
            expect(appHighlightStyle.style([tag])).toBe(functionClass)
        }
        expect(appHighlightStyle.style([tags.definition(tags.variableName)])).not.toBe(functionClass)
        expect(appHighlightStyle.style([tags.propertyName])).not.toBe(functionClass)
        expect(appHighlightStyle.style([tags.definition(tags.typeName)])).toBe(appHighlightStyle.style([tags.typeName]))
    })

    it("colors real TypeScript function declarations, calls and methods while preserving variable/type roles", () => {
        const doc = 'function greet(): string { return "hello" }; const count: number = 1; class Greeter { method() { return greet() } }; const obj = new Greeter(); obj.method();'
        const state = EditorState.create({ doc, extensions: [javascript({ typescript: true })] })
        const tree = ensureSyntaxTree(state, doc.length, 1000)!
        const spans: { text: string; style: string }[] = []
        highlightTree(tree, appHighlightStyle, (from, to, style) => spans.push({ text: doc.slice(from, to), style }))
        const functionClass = appHighlightStyle.style([tags.function(tags.variableName)])
        expect(spans.filter((span) => span.text === "greet").map((span) => span.style)).toEqual([functionClass, functionClass])
        // The upstream JS grammar labels method declarations as generic property
        // definitions; do not recolor every object property just to mimic semantics.
        expect(spans.filter((span) => span.text === "method").map((span) => span.style)).toEqual([appHighlightStyle.style([tags.propertyName]), functionClass])
        expect(spans.find((span) => span.text === "count")?.style).toBe(appHighlightStyle.style([tags.definition(tags.variableName)]))
        for (const name of ["string", "number", "Greeter"]) {
            expect(spans.find((span) => span.text === name)?.style).toBe(appHighlightStyle.style([tags.typeName]))
        }
    })
})

it.each([
    ["yuzora", false, "#2456cc", "#b5830a"], ["yuzora", true, "#82b4ff", "#e0b341"],
    ["github", false, "#8250df", "#953800"], ["github", true, "#d2a8ff", "#ffa657"],
    ["one", false, "#4078f2", "#986801"], ["one", true, "#61afef", "#e5c07b"],
] as const)("keeps function and type roles backed by %s palette (dark=%s)", (theme, dark, functionColor, typeColor) => {
    const style = document.createElement("style")
    style.dataset.themeTest = ""
    style.textContent = paletteCss
    document.head.append(style)
    document.documentElement.classList.toggle("dark", dark)
    const editor = document.createElement("div")
    editor.className = "cm-editor"
    editor.dataset.syntaxTheme = theme
    document.body.append(editor)
    const css = getComputedStyle(editor)
    expect(css.getPropertyValue("--syn-func").trim()).toBe(functionColor)
    expect(css.getPropertyValue("--syn-type").trim()).toBe(typeColor)
    for (const tag of [tags.function(tags.variableName), tags.function(tags.definition(tags.variableName)), tags.function(tags.propertyName), tags.function(tags.definition(tags.propertyName))]) {
        const spec = appHighlightStyle.specs.find((item) => (Array.isArray(item.tag) ? item.tag : [item.tag]).includes(tag))
        expect(spec?.color).toBe("var(--syn-func)")
    }
})

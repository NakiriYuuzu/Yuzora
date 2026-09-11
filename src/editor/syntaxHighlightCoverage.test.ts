import { describe, expect, it } from "vitest"
import { EditorState } from "@codemirror/state"
import { ensureSyntaxTree } from "@codemirror/language"
import { highlightTree, tags } from "@lezer/highlight"
import { syntaxSamples, syntaxCoverage, type RenderedRole } from "../../fixtures/syntax-highlight-corpus"
import { languageExtensionFromPath } from "./cmExtensions"
import { appHighlightStyle } from "./cmTheme"

const roleTags = {
    keyword: tags.keyword, string: tags.string, comment: tags.comment, number: tags.number,
    function: tags.function(tags.variableName), type: tags.typeName, property: tags.propertyName,
    tag: tags.tagName, heading: tags.heading, link: tags.link, variable: tags.variableName, meta: tags.meta, definition: tags.definition(tags.variableName),
} satisfies Record<Exclude<RenderedRole, "plain">, unknown>

function highlighted(code: string, path: string) {
    const extension = languageExtensionFromPath(path)
    expect(extension, `${path} must resolve a language parser`).not.toBeNull()
    const state = EditorState.create({ doc: code, extensions: [extension!] })
    const tree = ensureSyntaxTree(state, code.length, 1000)
    expect(tree, `${path} must parse the complete sample`).not.toBeNull()
    const spans: { from: number; to: number; style: string; text: string }[] = []
    highlightTree(tree!, appHighlightStyle, (from, to, style) => spans.push({ from, to, style, text: code.slice(from, to) }))
    return { spans, tree: tree! }
}

describe("production syntax highlight corpus", () => {
    it("covers all 29 requested language families", () => {
        expect(syntaxSamples).toHaveLength(29)
        expect(new Set(syntaxSamples.map(sample => sample.id)).size).toBe(29)
        expect(syntaxCoverage.every(sample => Object.values(sample.roles).every(role => role.status !== "supported" || role.probes.every(probe => probe.hit)))).toBe(true)
    })
    for (const sample of syntaxSamples) {
        it.each(sample.files)(`${sample.name} %s gives applicable tokens their production style`, path => {
            const { spans, tree } = highlighted(sample.code, path)
            const errors: string[] = []
            tree.iterate({ enter: node => { if (node.type.isError) errors.push(sample.code.slice(node.from, node.to)) } })
            expect(errors, `${path} syntax errors`).toEqual([])
            for (const probe of sample.probes) {
                const from = sample.code.indexOf(probe.text)
                expect(from).toBeGreaterThanOrEqual(0)
                const role = probe.renderedAs ?? probe.role
                const expected = role === "plain" ? null : appHighlightStyle.style([roleTags[role]])
                const actual = spans.filter(span => span.to > from && span.from < from + probe.text.length)
                expect.soft(Array.from({ length: probe.text.length }, (_, offset) => from + offset)
                    .every(position => expected === null ? !actual.some(span => span.from <= position && span.to > position) : actual.some(span => span.from <= position && span.to > position && span.style === expected)),
                `${path}: ${probe.role} ${JSON.stringify(probe.text)} expected ${expected}; actual ${JSON.stringify(actual)}`).toBe(true)
            }
        })
    }

    it.each([
        ['tsx', 'interface Props { count: number }; const node = <strong title="hello">{42}</strong>;', 'scss', '$gap: 12px; .card { padding: $gap; }'],
        ['jsx', 'const node = <strong title="hello">{42}</strong>;', 'less', '@gap: 12px; .card { padding: @gap; }'],
        ['typescript', 'interface Props { count: number }; const title: string = "hello";', 'sass', '.card\n  padding: 12px'],
    ])("Vue embeds %s scripts and %s styles without treating them as plain HTML", (scriptLang, script, styleLang, style) => {
        const code = `<script lang="${scriptLang}">${script}</script><template><section>{{ 7 }}</section></template><style lang="${styleLang}">${style}</style>`
        const { tree, spans } = highlighted(code, "App.vue")
        const errors: string[] = []
        tree.iterate({ enter: node => { if (node.type.isError) errors.push(code.slice(node.from, node.to)) } })
        expect(errors).toEqual([])
        for (const [text, tag] of [["padding", tags.propertyName], ["12", tags.number], ['"hello"', tags.string], ["section", tags.tagName]] as const) {
            const from = code.indexOf(text)
            expect(spans.some(span => span.from <= from && span.to >= from + text.length && span.style === appHighlightStyle.style([tag])), text).toBe(true)
        }
        if (scriptLang === "tsx" || scriptLang === "jsx") expect(spans.some(span => span.text === "strong" && span.style === appHighlightStyle.style([tags.tagName]))).toBe(true)
        if (scriptLang !== "jsx") expect(spans.some(span => span.text === "Props" && span.style === appHighlightStyle.style([tags.typeName]))).toBe(true)
    })

    it("does not color ordinary fields, variables or CSS properties as functions", () => {
        for (const [path, code, plain, callable] of [
            ["sample.cpp", "class Greeter { int field; int greet() { return field; } };", "field", "greet"],
            ["sample.css", ".card { width: calc(100px); }", "width", "calc"],
            ["sample.scss", "$width: 10px; .card { width: calc($width); }", "width", "calc"],
        ]) {
            const { spans } = highlighted(code, path)
            const functionStyle = appHighlightStyle.style([tags.function(tags.variableName)])
            expect(spans.filter(span => span.text === plain).every(span => span.style !== functionStyle)).toBe(true)
            expect(spans.some(span => span.text === callable && span.style === functionStyle)).toBe(true)
        }
    })

    it("reports the Svelte SCSS limitation separately from successful TS and plain CSS tokens", () => {
        const coverage = syntaxCoverage.find(sample => sample.id === "svelte")!
        expect(coverage.status).toBe("partial")
        expect(coverage.limitations).toContainEqual(expect.objectContaining({ scope: "style-preprocessors", status: "partial" }))
        const { tree } = highlighted('<style lang="scss">$accent: red; .card { color: $accent; }</style>', "App.svelte")
        const errors: number[] = []
        tree.iterate({ enter: node => { if (node.type.isError) errors.push(node.from) } })
        // This installed upstream grammar dispatches SCSS to CSS. Do not turn
        // the passing plain-CSS sample into an unsupported full-Svelte claim.
        expect(errors.length).toBeGreaterThan(0)
    })
})

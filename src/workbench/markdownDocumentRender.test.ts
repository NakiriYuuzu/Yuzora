import { expect, it } from "vitest"
import { renderMarkdownDocument } from "./markdownDocumentRender"

it("resolves reference links across the whole document, including escaped labels", () => {
    const source = "[text][foo\\]bar]\n\n" + "ordinary paragraph\n\n".repeat(8000) + "[foo\\]bar]: https://example.com\n"
    const batches = renderMarkdownDocument(source)
    expect(batches[0].html).toContain('<a href="https://example.com">text</a>')
    expect(batches.length).toBeGreaterThan(1)
    expect(batches.map((batch) => batch.html).join("").match(/ordinary paragraph/g)).toHaveLength(8000)
})

it("splits a giant code block without dropping or altering any content", () => {
    const code = "<>& code line 🚀\n".repeat(20000) + "LAST CODE LINE\n"
    const batches = renderMarkdownDocument("```text\n" + code + "```\n\n# After code\n")
    const codeBatches = batches.filter((batch) => batch.code)
    expect(codeBatches.length).toBeGreaterThan(1)
    expect(codeBatches.every((batch) => batch.html.length < 200000)).toBe(true)
    const recovered = codeBatches.map((batch) => {
        const element = document.createElement("div")
        element.innerHTML = batch.html
        return element.textContent
    }).join("")
    expect(recovered).toBe(code)
    expect(batches.at(-1)?.html).toContain("After code")
})

it("keeps nested lists and tables whole and retains a giant paragraph", () => {
    const paragraph = "unbroken".repeat(20000)
    const batches = renderMarkdownDocument("- one\n  - nested\n- two\n\n| A | B |\n|---|---|\n| x | y |\n\n" + paragraph)
    const html = batches.map((batch) => batch.html).join("")
    expect(html).toContain("<table>")
    expect(html.match(/<ul>/g)).toHaveLength(2)
    expect(html).toContain(paragraph)
})

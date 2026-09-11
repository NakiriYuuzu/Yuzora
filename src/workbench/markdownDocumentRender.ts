import MarkdownIt from "markdown-it"

export interface MarkdownHtmlBatch { html: string; estimatedLines: number; code: boolean }
const md = new MarkdownIt({ html: true, linkify: true, breaks: false })
const BATCH_SIZE = 32 * 1024

/** One full parse keeps reference definitions available to every rendered block. */
export function renderMarkdownDocument(source: string): MarkdownHtmlBatch[] {
    const env = {}
    const tokens = md.parse(source, env)
    const batches: MarkdownHtmlBatch[] = []
    let html = ""
    let estimatedLines = 0
    const flush = () => {
        if (html) batches.push({ html, estimatedLines: Math.max(1, estimatedLines), code: false })
        html = ""; estimatedLines = 0
    }
    for (let i = 0; i < tokens.length;) {
        const token = tokens[i]
        if ((token.type === "fence" || token.type === "code_block") && token.content.length > BATCH_SIZE) {
            flush()
            for (let from = 0; from < token.content.length;) {
                let to = Math.min(from + BATCH_SIZE, token.content.length)
                // Keep lines whole where practical, and never split a surrogate pair.
                const lineEnd = token.content.lastIndexOf("\n", to - 1)
                if (lineEnd >= from) to = lineEnd + 1
                if (to < token.content.length && /[\uD800-\uDBFF]/.test(token.content[to - 1])) to--
                const chunk = token.content.slice(from, to)
                batches.push({ html: `<pre><code>${md.utils.escapeHtml(chunk)}</code></pre>`, estimatedLines: Math.max(1, chunk.split("\n").length - (chunk.endsWith("\n") ? 1 : 0)), code: true })
                from = to
            }
            i++
            continue
        }
        let end = i + 1
        if (token.nesting === 1) {
            let depth = 1
            while (end < tokens.length && depth > 0) { depth += tokens[end].nesting; end++ }
        }
        const block = tokens.slice(i, end)
        const rendered = md.renderer.render(block, md.options, env)
        if (html.length + rendered.length > BATCH_SIZE) flush()
        html += rendered
        estimatedLines += Math.max(2, Math.ceil(block.reduce((sum, item) => sum + item.content.length, 0) / 75) + 1)
        i = end
    }
    flush()
    return batches
}

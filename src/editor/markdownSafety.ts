import MarkdownIt from "markdown-it"

const parser = new MarkdownIt({ html: true })

/** Unknown Markdown must remain source-backed, never silently normalized away. */
export function needsMarkdownSource(content: string): boolean {
    if (/^(---|\+\+\+)\r?\n/.test(content) || /\[\^[^\]]+\]|\$\$|^:::|^\s*\[[^\]]+\]:/m.test(content)) return true
    const env: { references?: Record<string, { href: string; title: string }> } = {}
    const tokens = parser.parse(content, env)
    // Definitions disappear from rendered HTML, so render equivalence cannot
    // detect their deletion. Let the parser recognize escaped/multiline labels.
    if (env.references) return true
    return tokens.some((token) =>
        token.type === "html_block" || token.children?.some((child) =>
            child.type === "html_inline" || child.type === "image"
        )
    )
}

export function markdownRoundTripSafe(original: string, serialized: string): boolean {
    return !needsMarkdownSource(original) && parser.render(original) === parser.render(serialized)
}

import MarkdownIt from "markdown-it"

const parser = new MarkdownIt({ html: true })

/** Unknown Markdown must remain source-backed, never silently normalized away. */
export function needsMarkdownSource(content: string): boolean {
    if (/^(---|\+\+\+)\r?\n/.test(content) || /^\[\^.+\]:|\$\$|^:::|^\s*\[[^\]]+\]:/m.test(content)) return true
    return parser.parse(content, {}).some((token) =>
        token.type === "html_block" || token.children?.some((child) =>
            child.type === "html_inline" || child.type === "image"
        )
    )
}

export function markdownRoundTripSafe(original: string, serialized: string): boolean {
    return !needsMarkdownSource(original) && parser.render(original) === parser.render(serialized)
}

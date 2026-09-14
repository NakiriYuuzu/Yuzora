import MarkdownIt from 'markdown-it'

export type CopyLineEnding = 'lf' | 'crlf'
const markdown = new MarkdownIt({ html: false, linkify: false, typographer: false })

/** Clean selected terminal text, never serialize/rewrite the Markdown document.
 * Token ranges protect syntax whose whitespace is meaningful. Four-space/tab
 * indentation is intentionally ambiguous and remains an indented code block.
 */
export function formatTerminalSelection(selection: string, lineEnding: CopyLineEnding): string {
  const normalized = selection.replace(/\r\n?/g, '\n')
  if (!/\S/.test(normalized)) return ''
  const lines = normalized.split('\n')
  // 1 protects layout, 2 protects code bytes (apart from the requested EOL).
  const protectedLines = new Uint8Array(lines.length)
  const hardBreaks = new Uint8Array(lines.length)
  const protect = (from: number, to: number, mode: number) => {
    for (let i = from; i < to; i++) protectedLines[i] = Math.max(protectedLines[i], mode)
  }
  for (const token of markdown.parse(normalized, {})) {
    if (!token.map) continue
    const [from, to] = token.map
    if (token.type === 'fence' || token.type === 'code_block') protect(from, to, 2)
    else if (['bullet_list_open', 'ordered_list_open', 'blockquote_open', 'table_open'].includes(token.type)) protect(from, to, 1)
    else if (token.type === 'inline') {
      // A multiline code span's children do not expose source positions. Keep
      // its enclosing inline block verbatim rather than trim inside the span.
      if (token.children?.some(child => child.type === 'code_inline') && token.content.includes('\n') && /`[^`]*\n[^`]*`/.test(token.content)) protect(from, to, 2)
      if (token.children?.some(child => child.type === 'hardbreak')) {
        for (let i = from; i < to - 1; i++) if (/ {2,}$/.test(lines[i])) hardBreaks[i] = 1
      }
    }
  }
  // Terminal trees/column output are often plain Markdown paragraphs. Preserve
  // the whole paragraph when box drawing or aligned columns establish layout.
  let paragraph = 0
  for (let i = 0; i <= lines.length; i++) {
    if (i < lines.length && lines[i].trim()) continue
    const block = lines.slice(paragraph, i)
    const diagram = block.some(line => /[\u2500-\u257f]|^[ \t]*[+|][+|\-= ]+[+|][ \t]*$/.test(line))
    const columns = block.filter(line => /\S[ \t]{2,}\S/.test(line)).length >= 2
    if (diagram || columns) protect(paragraph, i, 1)
    paragraph = i + 1
  }
  let start = 0, end = lines.length
  while (start < end && !lines[start].trim()) start++
  while (end > start && !lines[end - 1].trim()) end--
  const result: string[] = []
  for (let i = start; i < end; i++) {
    const mode = protectedLines[i]
    let line = lines[i]
    if (mode !== 2) {
      line = line.replace(/[ \t]+$/g, '')
      if (!mode) line = line.replace(/^[ \t]+/g, '')
      if (hardBreaks[i]) line += '  '
    }
    if (!line.trim() && !mode && result.at(-1) === '') continue
    result.push(line)
  }
  return result.join(lineEnding === 'crlf' ? '\r\n' : '\n')
}

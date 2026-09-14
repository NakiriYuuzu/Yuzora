import type { Terminal } from '@xterm/xterm'

/** HERDR raster frames may materialize every right-margin cell as a space.
 * Remove that screen padding before Markdown sees it as a code-space or hard
 * break. Use public xterm ranges, never infer selection from the whole screen.
 */
export function terminalSelectionSnapshot(term: Terminal): string {
  const selected = term.getSelection()
  const range = term.getSelectionPosition?.()
  const buffer = term.buffer?.active
  if (!range || !buffer) return selected
  const original: string[] = [], clean: string[] = []
  for (let row = range.start.y; row <= range.end.y; row++) {
    const line = buffer.getLine(row)
    if (!line) return selected
    const from = row === range.start.y ? range.start.x : 0
    const to = row === range.end.y ? range.end.x : term.cols
    const text = line.translateToString(true, from, to).replace(/\u00a0/g, ' ')
    // Short trailing runs may be intentional hard breaks. Only a run filling
    // the right edge establishes screen padding; partial selections stay raw.
    const stripped = to >= term.cols ? text.replace(/[ \t]{3,}$/g, '') : text
    if (row !== range.start.y && line.isWrapped) {
      original[original.length - 1] += text
      // Spaces on a soft-wrapped line separate words and must not disappear.
      clean[clean.length - 1] += stripped
    } else { original.push(text); clean.push(stripped) }
    if (buffer.getLine(row + 1)?.isWrapped) clean[clean.length - 1] = original[original.length - 1]
  }
  // Column selections and adapter-specific selection modes must retain their
  // own semantics rather than being rebuilt as a normal rectangular range.
  return original.join('\n') === selected.replace(/\r\n?/g, '\n') ? clean.join('\n') : selected
}

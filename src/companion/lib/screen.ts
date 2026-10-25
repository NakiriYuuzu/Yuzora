import type { Terminal } from "@xterm/xterm"

/** The pane's visible rows as text, for the approval confirmation. */
export function screenText(terminal: Pick<Terminal, "rows" | "buffer"> | null): string {
  if (!terminal) return ""
  const buffer = terminal.buffer.active
  const lines: string[] = []
  for (let row = 0; row < terminal.rows; row += 1) lines.push(buffer.getLine(buffer.viewportY + row)?.translateToString(true) ?? "")
  while (lines.length > 0 && lines[lines.length - 1].trim() === "") lines.pop()
  return lines.join("\n")
}

const encoder = new TextEncoder()
export const byteLength = (text: string) => encoder.encode(text).length

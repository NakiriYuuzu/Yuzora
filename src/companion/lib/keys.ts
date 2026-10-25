import type { AgentKey } from "./protocol"

export const KEY_BAR: { key: AgentKey; label: string }[] = [
  { key: "Escape", label: "Esc" },
  { key: "Tab", label: "Tab" },
  { key: "BackTab", label: "⇧Tab" },
  { key: "Ctrl-C", label: "^C" },
  { key: "Ctrl-D", label: "^D" },
  { key: "ArrowUp", label: "↑" },
  { key: "ArrowDown", label: "↓" },
  { key: "ArrowLeft", label: "←" },
  { key: "ArrowRight", label: "→" },
  { key: "Enter", label: "⏎" },
]

const ARROWS: Partial<Record<AgentKey, string>> = { ArrowUp: "A", ArrowDown: "B", ArrowRight: "C", ArrowLeft: "D" }
const BYTES: Partial<Record<AgentKey, string>> = { Enter: "\r", Escape: "\x1b", Tab: "\t", BackTab: "\x1b[Z", "Ctrl-C": "\x03", "Ctrl-D": "\x04" }

/**
 * The bytes a key sends over a control stream. Arrows follow the pane's
 * DECCKM mode, which the local xterm tracks from the same output.
 */
export function keyBytes(key: AgentKey, applicationCursor: boolean): string {
  const arrow = ARROWS[key]
  if (arrow) return `\x1b${applicationCursor ? "O" : "["}${arrow}`
  return BYTES[key]!
}

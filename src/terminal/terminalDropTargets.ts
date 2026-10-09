/** Where a file drag can paste paths: one entry per mounted HERDR terminal leaf. */
export interface TerminalDropTarget {
  /** Runtime scope (`contextSessionName`) the terminal belongs to. */
  scope: string
  canWrite(): boolean
  paste(text: string): Promise<void>
  focus(): void
}

const targets = new Map<string, TerminalDropTarget>()

export function registerTerminalDropTarget(key: string, target: TerminalDropTarget): () => void {
  targets.set(key, target)
  return () => {
    if (targets.get(key) === target) targets.delete(key)
  }
}

/** Resolves the terminal leaf under an element via its `data-attachment-key`. */
export function terminalDropTargetAt(element: Element | null): TerminalDropTarget | null {
  const key = element?.closest("[data-attachment-key]")?.getAttribute("data-attachment-key")
  return key ? targets.get(key) ?? null : null
}

export interface TerminalViewHandle {
  hasSelection: () => boolean
  getSelection: () => string
  isReady?: () => boolean
  paste: (text: string) => void | Promise<void>
  clear: () => void
}

const terminalViews = new Map<string, TerminalViewHandle>()

export function registerTerminalView(
  sessionId: string,
  handle: TerminalViewHandle
): () => void {
  terminalViews.set(sessionId, handle)
  return () => {
    if (terminalViews.get(sessionId) === handle) terminalViews.delete(sessionId)
  }
}

export function getTerminalView(sessionId: string): TerminalViewHandle | undefined {
  return terminalViews.get(sessionId)
}

import { formatTerminalSelection, type CopyLineEnding } from './terminalCopyFormat'

self.onmessage = (event: MessageEvent<{ id: number; text: string; lineEnding: CopyLineEnding }>) => {
  const { id, text, lineEnding } = event.data
  try {
    self.postMessage({ id, text: formatTerminalSelection(text, lineEnding) })
  } catch {
    // Never put selected terminal content in errors or diagnostics.
    self.postMessage({ id, error: 'clipboard-format-failed' })
  }
}

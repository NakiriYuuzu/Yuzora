import { readText, writeText } from "@tauri-apps/plugin-clipboard-manager"
import type { IDisposable, Terminal } from "@xterm/xterm"

export interface TerminalClipboardController extends IDisposable {
  flushPendingPaste: () => void
}

interface TerminalClipboardOptions {
  canPaste?: () => boolean
}

function browserClipboard(): Clipboard | null {
  if (typeof navigator === "undefined") return null
  return navigator.clipboard ?? null
}

async function writeClipboardText(text: string): Promise<void> {
  try {
    await writeText(text)
    return
  } catch (pluginError) {
    const clipboard = browserClipboard()
    if (!clipboard?.writeText) throw pluginError
    await clipboard.writeText(text)
  }
}

async function readClipboardText(): Promise<string> {
  try {
    return await readText()
  } catch (pluginError) {
    const clipboard = browserClipboard()
    if (!clipboard?.readText) throw pluginError
    return clipboard.readText()
  }
}

/**
 * Owns terminal copy/paste across xterm keyboard, menu ClipboardEvents, and
 * Tauri WebView clipboard fallbacks. Copy remains available in observe mode;
 * callers gate only paste delivery through `canPaste`.
 */
export function installTerminalClipboardHandling(
  term: Terminal,
  options: TerminalClipboardOptions = {}
): TerminalClipboardController {
  let disposed = false
  let pendingPaste: string | null = null
  const element = term.element
  const textarea = term.textarea

  const canPaste = () => !disposed && (options.canPaste?.() ?? true)

  const deliverPaste = (text: string) => {
    if (disposed || text.length === 0) return
    if (!canPaste()) {
      pendingPaste = text
      return
    }
    pendingPaste = null
    term.paste(text)
  }

  const copySelection = () => {
    if (disposed || !term.hasSelection()) return
    void writeClipboardText(term.getSelection()).catch(() => undefined)
  }

  const pasteClipboard = () => {
    if (disposed) return
    void readClipboardText()
      .then(deliverPaste)
      .catch(() => undefined)
  }

  const handleShortcut = (event: KeyboardEvent): boolean => {
    if (
      event.type !== "keydown"
      || event.altKey
      || (!event.ctrlKey && !event.metaKey)
    ) return true

    const key = event.key.toLowerCase()
    if (key === "c") {
      if (!term.hasSelection()) return true
      event.preventDefault()
      copySelection()
      return false
    }
    if (key === "v") {
      event.preventDefault()
      pasteClipboard()
      return false
    }
    return true
  }

  term.attachCustomKeyEventHandler(handleShortcut)

  const handleKeyDown = (event: KeyboardEvent) => {
    if (handleShortcut(event)) return
    event.stopImmediatePropagation()
  }
  const handleCopy = (event: ClipboardEvent) => {
    if (!term.hasSelection()) return
    const selection = term.getSelection()
    event.preventDefault()
    event.stopImmediatePropagation()
    event.clipboardData?.setData("text/plain", selection)
    void writeClipboardText(selection).catch(() => undefined)
  }
  const handlePaste = (event: ClipboardEvent) => {
    event.preventDefault()
    event.stopImmediatePropagation()
    const text = event.clipboardData?.getData("text/plain") ?? ""
    if (text.length > 0) {
      deliverPaste(text)
      return
    }
    pasteClipboard()
  }

  element?.addEventListener("keydown", handleKeyDown, true)
  element?.addEventListener("copy", handleCopy, true)
  element?.addEventListener("paste", handlePaste, true)
  if (textarea && textarea !== element) {
    textarea.addEventListener("paste", handlePaste, true)
  }

  return {
    flushPendingPaste: () => {
      if (pendingPaste !== null) deliverPaste(pendingPaste)
    },
    dispose: () => {
      disposed = true
      pendingPaste = null
      element?.removeEventListener("keydown", handleKeyDown, true)
      element?.removeEventListener("copy", handleCopy, true)
      element?.removeEventListener("paste", handlePaste, true)
      if (textarea && textarea !== element) {
        textarea.removeEventListener("paste", handlePaste, true)
      }
    }
  }
}

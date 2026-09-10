import { readText, writeText } from "@tauri-apps/plugin-clipboard-manager"
import type { IDisposable, Terminal } from "@xterm/xterm"

export interface TerminalClipboardController extends IDisposable {
  flushPendingPaste: () => void
}

interface TerminalClipboardOptions {
  canPaste?: () => boolean
  pasteText?: (text: string) => void
  pasteImage?: (image?: Blob) => void
  copyOnSelect?: () => boolean
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
  let initialConnectionSetup = true
  let initialConnectionWasWritable: boolean | null = null
  const element = term.element
  const textarea = term.textarea
  let selecting = false
  let selectionTimer: ReturnType<typeof setTimeout> | undefined

  const canPaste = () => !disposed && (options.canPaste?.() ?? true)

  const deliverPaste = (text: string) => {
    if (disposed || text.length === 0) return
    if (!canPaste()) {
      if (initialConnectionSetup) pendingPaste = text
      return
    }
    pendingPaste = null
    if (options.pasteText) options.pasteText(text)
    else term.paste(text)
  }

  const copySelection = () => {
    if (disposed || !term.hasSelection()) return
    void writeClipboardText(term.getSelection()).catch(() => undefined)
  }

  const pasteClipboard = () => {
    if (disposed) return
    const requestedDuringInitialSetup = initialConnectionSetup
    if (!requestedDuringInitialSetup && !canPaste()) return
    void readClipboardText()
      .then((text) => {
        if (
          requestedDuringInitialSetup
          && !initialConnectionSetup
          && initialConnectionWasWritable !== true
        ) return
        deliverPaste(text)
      })
      .catch(() => undefined)
  }

  const handleShortcut = (event: KeyboardEvent): boolean => {
    if (event.type === "keydown" && event.key === "Enter" && event.shiftKey
      && !event.ctrlKey && !event.altKey && !event.metaKey && !event.isComposing && event.keyCode !== 229) {
      event.preventDefault()
      // A raw CR/LF can submit the agent prompt. Use HERDR's atomic paste
      // boundary to insert a literal newline, independent of xterm's mode.
      if (canPaste()) deliverPaste("\n")
      return false
    }
    if (event.type === "keydown" && event.altKey && !event.ctrlKey && !event.metaKey
      && (event.code === "KeyV" || event.key.toLowerCase() === "v") && options.pasteImage) {
      event.preventDefault()
      if (canPaste() && !event.repeat) options.pasteImage()
      return false
    }
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

  // A few embedded/xterm test adapters expose only the DOM event surface.
  // Production xterm supports this API; preserve clipboard behavior through
  // the capture listeners when the adapter omits it.
  term.attachCustomKeyEventHandler?.(handleShortcut)

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
    const image = Array.from(event.clipboardData?.items ?? []).find((item) => item.type === "image/png")?.getAsFile()
    if (image && options.pasteImage) {
      if (canPaste()) options.pasteImage(image)
      return
    }
    const text = event.clipboardData?.getData("text/plain") ?? ""
    if (text.length > 0) {
      deliverPaste(text)
      return
    }
    pasteClipboard()
  }

  const handleMouseDown = (event: MouseEvent) => { selecting = event.button === 0 }
  const handleMouseUp = () => {
    if (!selecting) return
    selecting = false
    if (selectionTimer) clearTimeout(selectionTimer)
    selectionTimer = setTimeout(() => {
      if (!disposed && options.copyOnSelect?.()) copySelection()
    }, 0)
  }

  element?.addEventListener("keydown", handleKeyDown, true)
  element?.addEventListener("copy", handleCopy, true)
  element?.addEventListener("paste", handlePaste, true)
  element?.addEventListener("mousedown", handleMouseDown, true)
  window.addEventListener("mouseup", handleMouseUp)
  if (textarea && textarea !== element) {
    textarea.addEventListener("paste", handlePaste, true)
  }

  return {
    flushPendingPaste: () => {
      const text = pendingPaste
      pendingPaste = null
      if (initialConnectionSetup) initialConnectionWasWritable = canPaste()
      initialConnectionSetup = false
      if (text !== null && canPaste()) deliverPaste(text)
    },
    dispose: () => {
      disposed = true
      pendingPaste = null
      if (selectionTimer) clearTimeout(selectionTimer)
      element?.removeEventListener("mousedown", handleMouseDown, true)
      window.removeEventListener("mouseup", handleMouseUp)
      element?.removeEventListener("keydown", handleKeyDown, true)
      element?.removeEventListener("copy", handleCopy, true)
      element?.removeEventListener("paste", handlePaste, true)
      if (textarea && textarea !== element) {
        textarea.removeEventListener("paste", handlePaste, true)
      }
    }
  }
}

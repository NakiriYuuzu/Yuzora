import type { IDisposable, Terminal } from "@xterm/xterm"

function syncTextareaToCursor(term: Terminal): void {
    const textarea = term.textarea
    const screen = term.element?.querySelector<HTMLElement>(".xterm-screen")
    if (!textarea || !screen || term.cols <= 0 || term.rows <= 0) return

    const bounds = screen.getBoundingClientRect()
    if (bounds.width <= 0 || bounds.height <= 0) return

    const buffer = term.buffer.active
    const cursorLine = buffer.baseY + buffer.cursorY
    const viewportEnd = buffer.viewportY + term.rows
    if (cursorLine < buffer.viewportY || cursorLine >= viewportEnd) return

    const cellWidth = bounds.width / term.cols
    const cellHeight = bounds.height / term.rows
    if (!Number.isFinite(cellWidth) || !Number.isFinite(cellHeight)) return

    const cursorX = Math.min(buffer.cursorX, term.cols - 1)
    const cursorY = cursorLine - buffer.viewportY
    const cursorCellWidth = Math.max(buffer.getLine(cursorLine)?.getCell(cursorX)?.getWidth() ?? 1, 1)

    textarea.style.left = `${cursorX * cellWidth}px`
    textarea.style.top = `${cursorY * cellHeight}px`
    textarea.style.width = `${cursorCellWidth * cellWidth}px`
    textarea.style.height = `${cellHeight}px`
    textarea.style.lineHeight = `${cellHeight}px`
    textarea.style.zIndex = "-5"
}

/**
 * Backports the xterm composition/resize cursor sync fixes that landed after
 * 6.0.0, using only xterm's public DOM and buffer APIs.
 */
export function installTerminalImePositioning(term: Terminal): IDisposable {
    const textarea = term.textarea
    if (!textarea) return { dispose: () => undefined }

    let composing = false
    const handleCompositionStart = () => {
        syncTextareaToCursor(term)
        composing = true
    }
    const handleCompositionEnd = () => {
        composing = false
    }
    const handleBlur = () => {
        composing = false
    }
    const syncWhenIdle = () => {
        if (!composing) syncTextareaToCursor(term)
    }

    // Capture runs before xterm's own target listener, so WebView2 receives the
    // current anchor before it opens the native IME candidate window.
    textarea.addEventListener("compositionstart", handleCompositionStart, true)
    textarea.addEventListener("compositionend", handleCompositionEnd, true)
    textarea.addEventListener("blur", handleBlur, true)
    const resizeDisposable = term.onResize(syncWhenIdle)
    const scrollDisposable = term.onScroll(syncWhenIdle)

    return {
        dispose: () => {
            textarea.removeEventListener("compositionstart", handleCompositionStart, true)
            textarea.removeEventListener("compositionend", handleCompositionEnd, true)
            textarea.removeEventListener("blur", handleBlur, true)
            resizeDisposable.dispose()
            scrollDisposable.dispose()
        }
    }
}

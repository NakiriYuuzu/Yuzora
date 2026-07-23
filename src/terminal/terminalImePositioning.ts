import type { IDisposable, Terminal } from "@xterm/xterm"

export type TerminalImeAnchorMode = "cursor" | "tui"

export interface TerminalImePositioningOptions {
    anchorMode?: TerminalImeAnchorMode
}

function terminalScreen(term: Terminal): HTMLElement | null {
    return term.element?.querySelector<HTMLElement>(".xterm-screen") ?? null
}

function syncTextareaToCursor(term: Terminal): void {
    const textarea = term.textarea
    const screen = terminalScreen(term)
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

function createTuiOverlay(term: Terminal): HTMLElement | null {
    const screen = terminalScreen(term)
    if (!screen) return null

    const overlay = document.createElement("div")
    overlay.dataset.yuzoraImeOverlay = ""
    overlay.setAttribute("aria-hidden", "true")
    Object.assign(overlay.style, {
        position: "absolute",
        display: "none",
        minWidth: "24px",
        maxWidth: "calc(100% - 16px)",
        overflow: "hidden",
        padding: "0 4px",
        border: "1px solid var(--term-blue)",
        borderRadius: "3px",
        background: "var(--term-bg)",
        color: "var(--term-fg)",
        pointerEvents: "none",
        whiteSpace: "pre",
        zIndex: "4"
    })
    screen.append(overlay)
    return overlay
}

function syncTextareaToTuiOverlay(term: Terminal, overlay: HTMLElement): void {
    const textarea = term.textarea
    const screen = terminalScreen(term)
    if (!textarea || !screen || term.cols <= 0 || term.rows <= 0) return

    const bounds = screen.getBoundingClientRect()
    if (bounds.width <= 0 || bounds.height <= 0) return

    const cellWidth = bounds.width / term.cols
    const cellHeight = bounds.height / term.rows
    if (!Number.isFinite(cellWidth) || !Number.isFinite(cellHeight)) return

    const left = 8
    const top = Math.max(bounds.height - cellHeight - 8, 0)
    overlay.style.left = `${left}px`
    overlay.style.top = `${top}px`
    overlay.style.height = `${cellHeight}px`
    overlay.style.lineHeight = `${cellHeight}px`

    textarea.style.left = `${left}px`
    textarea.style.top = `${top}px`
    textarea.style.width = `${Math.max(overlay.getBoundingClientRect().width, cellWidth)}px`
    textarea.style.height = `${cellHeight}px`
    textarea.style.lineHeight = `${cellHeight}px`
    textarea.style.zIndex = "5"
}

/**
 * Backports the xterm composition/resize cursor sync fixes that landed after
 * 6.0.0, using only xterm's public DOM and buffer APIs.
 */
export function installTerminalImePositioning(
    term: Terminal,
    options: TerminalImePositioningOptions = {}
): IDisposable {
    const textarea = term.textarea
    if (!textarea) return { dispose: () => undefined }

    const anchorMode = options.anchorMode ?? "cursor"
    const overlay = anchorMode === "tui" ? createTuiOverlay(term) : null
    let composing = false
    let reassertTimer: number | undefined
    const syncAnchor = () => {
        if (overlay) syncTextareaToTuiOverlay(term, overlay)
        else syncTextareaToCursor(term)
    }
    const handleCompositionStart = () => {
        if (overlay) overlay.style.display = "block"
        syncAnchor()
        composing = true
    }
    const handleCompositionUpdate = (event: CompositionEvent) => {
        if (!overlay) return
        overlay.textContent = event.data
        overlay.style.display = "block"
        syncAnchor()
    }
    const reassertTuiAnchor = () => {
        if (!overlay || !composing) return
        syncAnchor()
        if (reassertTimer !== undefined) window.clearTimeout(reassertTimer)
        reassertTimer = window.setTimeout(() => {
            reassertTimer = undefined
            if (composing) syncAnchor()
        }, 0)
    }
    const handleCompositionEnd = () => {
        composing = false
        if (overlay) {
            overlay.textContent = ""
            overlay.style.display = "none"
        }
    }
    const handleBlur = () => {
        composing = false
        if (overlay) {
            overlay.textContent = ""
            overlay.style.display = "none"
        }
    }
    const syncWhenIdle = () => {
        if (overlay || !composing) syncAnchor()
    }

    // Capture runs before xterm's own target listener, so WebView2 receives the
    // current anchor before it opens the native IME candidate window.
    textarea.addEventListener("compositionstart", handleCompositionStart, true)
    textarea.addEventListener("compositionupdate", handleCompositionUpdate, true)
    textarea.addEventListener("compositionupdate", reassertTuiAnchor)
    textarea.addEventListener("compositionend", handleCompositionEnd, true)
    textarea.addEventListener("blur", handleBlur, true)
    const resizeDisposable = term.onResize(syncWhenIdle)
    const scrollDisposable = term.onScroll(syncWhenIdle)

    return {
        dispose: () => {
            if (reassertTimer !== undefined) window.clearTimeout(reassertTimer)
            textarea.removeEventListener("compositionstart", handleCompositionStart, true)
            textarea.removeEventListener("compositionupdate", handleCompositionUpdate, true)
            textarea.removeEventListener("compositionupdate", reassertTuiAnchor)
            textarea.removeEventListener("compositionend", handleCompositionEnd, true)
            textarea.removeEventListener("blur", handleBlur, true)
            resizeDisposable.dispose()
            scrollDisposable.dispose()
            overlay?.remove()
        }
    }
}

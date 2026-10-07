import type { IDisposable, Terminal } from "@xterm/xterm"

import {
    installTerminalImePositioning,
    type TerminalImePositioningOptions
} from "./terminalImePositioning"

interface CompositionState {
    initialLength: number
    startOffset: number
    wholeValueReplacement: boolean
}

interface PendingCommit {
    commit: string
    queuedData: string[]
}

function stripCompositionEcho(data: string, commit: string): string {
    for (let start = 0; start < commit.length; start += 1) {
        const possibleEcho = commit.slice(start)
        if (data.startsWith(possibleEcho)) return data.slice(possibleEcho.length)
    }
    return data
}

interface PendingInsertion {
    data: string
    emitted: boolean
}

/**
 * Owns xterm's user-input subscription so Windows TSF composition can be
 * normalised before data crosses the PTY/SSH boundary.
 *
 * It also recovers IME direct insertions that xterm 6.0 drops (xterm.js
 * #5887/#6045): WKWebView delivers `input` before the keyCode 229 keydown, so
 * when the previous key is still held xterm's `_keyDownSeen` gate ignores the
 * input and its later textarea diff no longer sees the inserted text.
 */
export function installTerminalImeHandling(
    term: Terminal,
    onData: (data: string) => void,
    options: TerminalImePositioningOptions = {}
): IDisposable {
    const textarea = term.textarea
    const positioning = installTerminalImePositioning(term, options)
    if (!textarea) {
        const dataDisposable = term.onData(onData)
        return {
            dispose: () => {
                dataDisposable.dispose()
                positioning.dispose()
            }
        }
    }

    let composition: CompositionState | undefined
    let pending: PendingCommit | undefined
    let settleTimer: number | undefined
    let commitTimer: number | undefined
    let insertion: PendingInsertion | undefined
    // xterm's own recovery paths still own the text while these are pending:
    // a 229 keydown schedules a textarea diff, and compositionend schedules a
    // finaliser that also picks up characters typed right after the commit.
    let textareaDiffsPending = 0
    let compositionSettling = 0
    const timers = new Set<number>()
    let disposed = false

    const afterXtermTimers = (depth: number, run: () => void) => {
        const id = window.setTimeout(() => {
            timers.delete(id)
            if (depth > 1) afterXtermTimers(depth - 1, run)
            else run()
        }, 0)
        timers.add(id)
    }

    const finishPendingCommit = () => {
        if (!pending || disposed) return
        const current = pending
        pending = undefined
        onData(current.commit)
        current.queuedData.forEach(onData)
    }

    const handleCompositionStart = () => {
        finishPendingCommit()
        const selectionStart = textarea.selectionStart ?? textarea.value.length
        composition = {
            initialLength: textarea.value.length,
            startOffset: selectionStart,
            wholeValueReplacement: false
        }
    }

    const handleCompositionUpdate = () => {
        if (!composition || composition.startOffset === 0) return
        const selectionStart = textarea.selectionStart
        const selectionEnd = textarea.selectionEnd
        if (
            selectionStart === 0
            && selectionEnd >= composition.startOffset
            && selectionEnd >= composition.initialLength
        ) {
            composition.wholeValueReplacement = true
        }
    }

    const handleCompositionEnd = (event: CompositionEvent) => {
        compositionSettling += 1
        afterXtermTimers(2, () => { compositionSettling -= 1 })
        if (!composition?.wholeValueReplacement || event.data.length === 0) {
            composition = undefined
            return
        }

        pending = { commit: event.data, queuedData: [] }
        composition = undefined

        // This capture listener schedules first. xterm schedules its own
        // composition finaliser from the target listener, so the nested timer
        // runs after xterm has emitted (or swallowed) its offset-based payload.
        settleTimer = window.setTimeout(() => {
            settleTimer = undefined
            commitTimer = window.setTimeout(() => {
                commitTimer = undefined
                finishPendingCommit()
            }, 0)
        }, 0)
    }

    // These capture listeners are added after xterm's own capture listeners, so
    // they observe each event after xterm has already handled it.
    const handleKeyDown = (event: KeyboardEvent) => {
        if (event.keyCode !== 229 || composition || compositionSettling > 0) return
        textareaDiffsPending += 1
        afterXtermTimers(1, () => { textareaDiffsPending -= 1 })
    }

    // xterm sends space and A–Z from keypress without preventDefault, so the
    // browser still inserts the same text afterwards; xterm itself skips that
    // input via `_keyPressHandled`. Remember what this key's keypress emitted so
    // the recovery below does not send it a second time.
    let dataCount = 0
    let lastData = ""
    let countBeforeKeyPress = 0
    let keyPressData: string | undefined
    const element = term.element
    const handleKeyPressStart = () => { countBeforeKeyPress = dataCount }
    const handleKeyPress = () => {
        keyPressData = dataCount > countBeforeKeyPress ? lastData : undefined
    }
    const handleKeyUp = () => { keyPressData = undefined }

    const handleBeforeInput = (event: InputEvent) => {
        const echoesKeyPress = event.data !== null && event.data === keyPressData
        keyPressData = undefined
        insertion = event.inputType === "insertText" && event.data && !event.isComposing && !composition && !echoesKeyPress
            ? { data: event.data, emitted: false }
            : undefined
    }

    const handleInput = (event: Event) => {
        const current = insertion
        insertion = undefined
        if (!current || (event as InputEvent).data !== current.data || current.emitted) return
        if (composition || compositionSettling > 0 || textareaDiffsPending > 0) return
        handleData(current.data)
    }

    textarea.addEventListener("compositionstart", handleCompositionStart, true)
    textarea.addEventListener("compositionupdate", handleCompositionUpdate, true)
    textarea.addEventListener("compositionend", handleCompositionEnd, true)
    textarea.addEventListener("keydown", handleKeyDown, true)
    element?.addEventListener("keypress", handleKeyPressStart, true)
    textarea.addEventListener("keypress", handleKeyPress, true)
    textarea.addEventListener("keyup", handleKeyUp, true)
    textarea.addEventListener("beforeinput", handleBeforeInput, true)
    textarea.addEventListener("input", handleInput, true)

    const handleData = (data: string) => {
        if (!pending) {
            onData(data)
            return
        }

        const remainder = stripCompositionEcho(data, pending.commit)
        if (remainder.length > 0) pending.queuedData.push(remainder)
    }

    const dataDisposable = term.onData((data) => {
        dataCount += 1
        lastData = data
        if (insertion && data === insertion.data) insertion.emitted = true
        handleData(data)
    })

    return {
        dispose: () => {
            disposed = true
            if (settleTimer !== undefined) window.clearTimeout(settleTimer)
            if (commitTimer !== undefined) window.clearTimeout(commitTimer)
            timers.forEach((id) => window.clearTimeout(id))
            timers.clear()
            textarea.removeEventListener("compositionstart", handleCompositionStart, true)
            textarea.removeEventListener("compositionupdate", handleCompositionUpdate, true)
            textarea.removeEventListener("compositionend", handleCompositionEnd, true)
            textarea.removeEventListener("keydown", handleKeyDown, true)
            element?.removeEventListener("keypress", handleKeyPressStart, true)
            textarea.removeEventListener("keypress", handleKeyPress, true)
            textarea.removeEventListener("keyup", handleKeyUp, true)
            textarea.removeEventListener("beforeinput", handleBeforeInput, true)
            textarea.removeEventListener("input", handleInput, true)
            dataDisposable.dispose()
            positioning.dispose()
        }
    }
}

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

/**
 * Owns xterm's user-input subscription so Windows TSF composition can be
 * normalised before data crosses the PTY/SSH boundary.
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
    let disposed = false

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

    textarea.addEventListener("compositionstart", handleCompositionStart, true)
    textarea.addEventListener("compositionupdate", handleCompositionUpdate, true)
    textarea.addEventListener("compositionend", handleCompositionEnd, true)

    const dataDisposable = term.onData((data) => {
        if (!pending) {
            onData(data)
            return
        }

        const remainder = stripCompositionEcho(data, pending.commit)
        if (remainder.length > 0) pending.queuedData.push(remainder)
    })

    return {
        dispose: () => {
            disposed = true
            if (settleTimer !== undefined) window.clearTimeout(settleTimer)
            if (commitTimer !== undefined) window.clearTimeout(commitTimer)
            textarea.removeEventListener("compositionstart", handleCompositionStart, true)
            textarea.removeEventListener("compositionupdate", handleCompositionUpdate, true)
            textarea.removeEventListener("compositionend", handleCompositionEnd, true)
            dataDisposable.dispose()
            positioning.dispose()
        }
    }
}

import { useEffect } from "react"
import { getCurrentWindow } from "@tauri-apps/api/window"
import { getCurrentWebview } from "@tauri-apps/api/webview"
import { isTauri } from "@/lib/platform"
import { useTextInputDialogStore } from "@/state/textInputDialogStore"
import { activeTerminalFocusPath, focusActiveTerminal } from "@/terminal/terminalFocus"

const workbenchInput = ".cm-content, .xterm-helper-textarea, .tiptap[contenteditable=true]"
const editable = "input, textarea, [contenteditable=true]"

function available(element: HTMLElement): boolean {
    if (!element.isConnected || element.closest('[hidden], [inert], [aria-hidden="true"]')) return false
    for (let parent: HTMLElement | null = element; parent; parent = parent.parentElement) {
        const style = getComputedStyle(parent)
        if (style.display === "none" || style.visibility === "hidden") return false
    }
    return true
}

/** Restore the last editing surface when the native window regains focus.
 * Dialogs, fields and deliberate navigation keep ownership of keyboard focus. */
export function WorkbenchFocusBridge() {
    useEffect(() => {
        const initial = document.activeElement
        let last: HTMLElement | null = initial instanceof HTMLElement && initial.matches(workbenchInput) ? initial : null
        let preserveField: HTMLElement | null = null
        let disposed = false
        let timer: ReturnType<typeof setTimeout> | undefined
        let unlisten: (() => void) | undefined
        let generation = 0
        let nativeFocus: Promise<boolean> | undefined
        let nativeFocusGeneration = -1
        const cancel = () => { generation++; clearTimeout(timer) }
        const remember = (event: FocusEvent) => {
            const target = event.target
            if (!(target instanceof HTMLElement)) return
            if (target.matches(workbenchInput)) {
                last = target
                preserveField = null
            } else if (target.matches(editable)) { preserveField = target; cancel() }
        }
        const blocked = () => {
            if (disposed || useTextInputDialogStore.getState().pending ||
                document.querySelector('[aria-modal="true"]:not([data-state="closed"]), dialog[open], [role="menu"][data-state="open"]')) return true
            const active = document.activeElement
            return Boolean(preserveField && available(preserveField) && active === preserveField)
                || active instanceof HTMLElement && active.matches(editable) && !active.matches(workbenchInput)
        }
        const restore = () => {
            clearTimeout(timer)
            const intent = generation
            timer = setTimeout(() => { void (async () => {
                if (intent !== generation || blocked()) return
                const path = activeTerminalFocusPath()
                if (path && isTauri() && !document.hasFocus()) {
                    // On WebView2, activating the outer window does not always
                    // give its WebView keyboard ownership. DOM focus alone can
                    // leave the xterm textarea selected but unable to receive keys.
                    if (!nativeFocus || nativeFocusGeneration !== intent) {
                        nativeFocusGeneration = intent
                        const request = (async () => {
                            if (!await getCurrentWindow().isFocused() || intent !== generation || blocked()
                                || activeTerminalFocusPath() !== path) return false
                            await getCurrentWebview().setFocus()
                            return true
                        })().finally(() => { if (nativeFocus === request) nativeFocus = undefined })
                        nativeFocus = request
                    }
                    if (!await nativeFocus) return
                    if (intent !== generation || blocked() || activeTerminalFocusPath() !== path) return
                }
                const focusInput = (remaining: number) => {
                    if (intent !== generation || blocked() || activeTerminalFocusPath() !== path) return
                    if (path && isTauri() && !document.hasFocus()) {
                        if (remaining > 0) timer = setTimeout(() => focusInput(remaining - 1), 50)
                        return
                    }
                    if (focusActiveTerminal(path)) return
                    const active = document.activeElement
                    if (!last || !available(last)) return
                    if (active && active !== document.body && active !== document.documentElement && active !== last) return
                    last.focus({ preventScroll: true })
                }
                focusInput(10)
            })().catch(() => undefined)
            }, 0)
        }
        document.addEventListener("focusin", remember)
        document.addEventListener("pointerdown", cancel, true)
        window.addEventListener("focus", restore)
        window.addEventListener("blur", cancel)
        if (isTauri()) {
            void getCurrentWindow().onFocusChanged(({ payload }) => {
                if (payload) restore()
                else cancel()
            }).then((release) => { if (disposed) release(); else unlisten = release })
                .catch(() => undefined)
        }
        return () => {
            disposed = true
            clearTimeout(timer)
            unlisten?.()
            document.removeEventListener("focusin", remember)
            document.removeEventListener("pointerdown", cancel, true)
            window.removeEventListener("focus", restore)
            window.removeEventListener("blur", cancel)
        }
    }, [])
    return null
}

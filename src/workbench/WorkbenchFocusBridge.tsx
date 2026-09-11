import { useEffect } from "react"
import { getCurrentWindow } from "@tauri-apps/api/window"
import { isTauri } from "@/lib/platform"
import { useTextInputDialogStore } from "@/state/textInputDialogStore"

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
        let preserveField = false
        let disposed = false
        let timer: ReturnType<typeof setTimeout> | undefined
        let unlisten: (() => void) | undefined
        const remember = (event: FocusEvent) => {
            const target = event.target
            if (!(target instanceof HTMLElement)) return
            if (target.matches(workbenchInput)) {
                last = target
                preserveField = false
            } else if (target.matches(editable)) preserveField = true
        }
        const restore = () => {
            clearTimeout(timer)
            timer = setTimeout(() => {
                if (disposed || preserveField || !last || !available(last)) return
                if (useTextInputDialogStore.getState().pending ||
                    document.querySelector('[aria-modal="true"]:not([data-state="closed"]), dialog[open], [role="menu"][data-state="open"]')) return
                const active = document.activeElement
                if (active && active !== document.body && active !== document.documentElement && active !== last) return
                last.focus({ preventScroll: true })
            }, 0)
        }
        document.addEventListener("focusin", remember)
        window.addEventListener("focus", restore)
        if (isTauri()) {
            void getCurrentWindow().onFocusChanged(({ payload }) => {
                if (payload) restore()
            }).then((release) => { if (disposed) release(); else unlisten = release })
                .catch(() => undefined)
        }
        return () => {
            disposed = true
            clearTimeout(timer)
            unlisten?.()
            document.removeEventListener("focusin", remember)
            window.removeEventListener("focus", restore)
        }
    }, [])
    return null
}

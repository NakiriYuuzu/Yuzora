import { useEffect } from "react"
import { getCurrentWindow } from "@tauri-apps/api/window"
import { getCurrentWebview } from "@tauri-apps/api/webview"
import { isTauri, isWindowsPlatform } from "@/lib/platform"
import { isWorkbenchWindowActive } from "@/lib/ipc"
import { useTextInputDialogStore } from "@/state/textInputDialogStore"
import { PREVIEW_TAB_PATH, useWorkspaceStore } from "@/state/workspaceStore"
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
        let unlistenActivation: (() => void) | undefined
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
        const focusNative = (intent: number, current: () => boolean) => {
            if (!nativeFocus || nativeFocusGeneration !== intent) {
                nativeFocusGeneration = intent
                const request = (async () => {
                    if (!await isWorkbenchWindowActive() || intent !== generation || disposed || !current()) return false
                    await getCurrentWebview().setFocus()
                    return true
                })().finally(() => { if (nativeFocus === request) nativeFocus = undefined })
                nativeFocus = request
            }
            return nativeFocus
        }
        const restore = (nativeActivation = false) => {
            clearTimeout(timer)
            const intent = generation
            timer = setTimeout(() => { void (async () => {
                if (intent !== generation || disposed) return
                const field = document.activeElement
                const workspace = useWorkspaceStore.getState()
                const groupIndex = workspace.activeGroupIndex
                const pagePath = workspace.groups[groupIndex]?.activePath
                const browserVisible = workspace.groups.some(group => group.activePath === PREVIEW_TAB_PATH
                    || group.tabs.some(tab => tab.path === group.activePath && tab.kind === "preview"))
                if (nativeActivation && isTauri() && field instanceof HTMLElement && field.matches(editable)
                    && !field.matches(".xterm-helper-textarea") && available(field)
                    && (!browserVisible || field.closest('[aria-modal="true"], dialog[open]'))) {
                    // Reclaim native keyboard ownership without changing the DOM
                    // field, its selection, or a dialog's focus trap. A visible
                    // child Browser can retain a stale main-document activeElement.
                    await focusNative(intent, () => {
                        const current = useWorkspaceStore.getState()
                        return document.activeElement === field && available(field)
                            && current.workspacePath === workspace.workspacePath
                            && current.activeGroupIndex === groupIndex
                            && current.groups[groupIndex]?.activePath === pagePath
                    })
                    return
                }
                if (blocked()) return
                const path = activeTerminalFocusPath()
                if (path && isTauri() && (nativeActivation || !document.hasFocus())) {
                    // On WebView2, activating the outer window does not always
                    // give its WebView keyboard ownership. DOM focus alone can
                    // leave the xterm textarea selected but unable to receive keys,
                    // even if document.hasFocus() still reports true.
                    if (!await focusNative(intent, () => !blocked() && activeTerminalFocusPath() === path)) return
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
        const restoreDocument = () => restore()
        document.addEventListener("focusin", remember)
        document.addEventListener("pointerdown", cancel, true)
        window.addEventListener("focus", restoreDocument)
        window.addEventListener("blur", cancel)
        if (isTauri()) {
            let windowActive = false
            const onActivation = ({ payload }: { payload: boolean }) => {
                if (payload === windowActive) return
                windowActive = payload
                if (payload) restore(true)
                else cancel()
            }
            void getCurrentWindow().onFocusChanged(event => {
                // Windows emits this for WebView GotFocus, including feedback
                // from our own setFocus call. Only the outer HWND activation
                // event may reacquire native focus there; otherwise the async
                // feedback starts an endless focus loop after each request.
                if (!isWindowsPlatform()) onActivation(event)
            })
                .then((release) => { if (disposed) release(); else unlisten = release })
                .catch(() => undefined)
            // Windows taskbar activation can precede (or omit) WebView GotFocus.
            // Observe the top-level HWND independently to restart restoration.
            void getCurrentWindow().listen<boolean>("workbench:window-activation", onActivation)
                .then((release) => { if (disposed) release(); else unlistenActivation = release })
                .catch(() => undefined)
        }
        return () => {
            disposed = true
            clearTimeout(timer)
            unlisten?.()
            unlistenActivation?.()
            document.removeEventListener("focusin", remember)
            document.removeEventListener("pointerdown", cancel, true)
            window.removeEventListener("focus", restoreDocument)
            window.removeEventListener("blur", cancel)
        }
    }, [])
    return null
}

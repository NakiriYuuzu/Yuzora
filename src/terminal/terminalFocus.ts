import { useWorkspaceStore } from "@/state/workspaceStore"
import { useTextInputDialogStore } from "@/state/textInputDialogStore"

interface FocusTarget {
    pagePath: string
    active: () => boolean
    focus: () => void
}
const targets = new Map<string, FocusTarget>()
let focusIntent = 0

export function registerTerminalFocusTarget(key: string, target: FocusTarget): () => void {
    targets.set(key, target)
    return () => { if (targets.get(key) === target) targets.delete(key) }
}

export function focusActiveTerminal(pagePath?: string): boolean {
    if (useTextInputDialogStore.getState().pending || document.querySelector('[aria-modal="true"]:not([data-state="closed"]), dialog[open], [role="menu"][data-state="open"]')) return false
    const state = useWorkspaceStore.getState()
    const activePath = state.groups[state.activeGroupIndex]?.activePath
    if (pagePath && pagePath !== activePath) return false
    const target = [...targets.values()].find((entry) => entry.pagePath === activePath && entry.active())
    if (!target) return false
    target.focus()
    return true
}

/** Wait for dialog teardown / terminal mounting without retaining a stale DOM node. */
export function requestTerminalFocus(pagePath: string): void {
    const intent = ++focusIntent
    const attempt = (remaining: number) => {
        if (intent !== focusIntent) return
        const state = useWorkspaceStore.getState()
        if (state.groups[state.activeGroupIndex]?.activePath !== pagePath) return
        const active = document.activeElement
        if (active instanceof Element && active.closest('input, textarea, [contenteditable="true"]') && !active.closest('.xterm, [data-state="closed"]')) return
        if (!focusActiveTerminal(pagePath) && remaining > 0) setTimeout(() => attempt(remaining - 1), 50)
    }
    setTimeout(() => attempt(100), 0)
}

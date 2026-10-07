import { Compartment, StateEffect } from "@codemirror/state"
import { EditorView } from "@codemirror/view"

import { canonicalPathKey, isWindowsPath } from "../lib/paths"

export interface EditorViewMetadata {
    groupIndex: number
    readonly: boolean
}

export interface RegisteredEditorView extends EditorViewMetadata {
    // The EditorView object is also the registry identity. All metadata updates
    // and unregisters are identity-guarded so a replaced pane cannot mutate the
    // newer view registered for the same path.
    view: EditorView
}

// Operational Windows paths register only an explicit Windows alias key. An
// A remote UNC path can decode to `//host/share/...`, which is
// ambiguous with a case-sensitive POSIX double-slash path: exact POSIX lookup
// wins, and only a view registered from unambiguous Windows syntax participates
// in the fallback Windows alias lookup.
const views = new Map<string, RegisteredEditorView>()

function automaticViewKey(path: string): string {
    return `auto:${canonicalPathKey(path)}`
}

function windowsAliasKey(path: string): string {
    return `windows:${canonicalPathKey(path, "windows")}`
}

function registrationKeys(path: string): string[] {
    // Unambiguous Windows operational paths live only in the Windows namespace.
    // Otherwise a lower-case UNC alias such as `\\server\share\file.ts` would
    // overwrite the exact, case-sensitive POSIX registration
    // `//server/share/file.ts` in the automatic namespace.
    return isWindowsPath(path)
        ? [windowsAliasKey(path)]
        : [automaticViewKey(path)]
}

function lookupKeys(path: string): string[] {
    // Unambiguous Windows syntax must never consult the exact POSIX namespace.
    if (isWindowsPath(path)) return [windowsAliasKey(path)]

    const keys = [automaticViewKey(path)]
    // A forward-slash `//host/share/...` path can come from a remote UNC path.
    // Prefer an exact POSIX registration, then fall back to a registered
    // Windows UNC operational path only when no exact POSIX entry exists.
    if (path.startsWith("//")) keys.push(windowsAliasKey(path))
    return keys
}

function findViewEntry(path: string): RegisteredEditorView | undefined {
    for (const key of lookupKeys(path)) {
        const entry = views.get(key)
        if (entry) return entry
    }
    return undefined
}

export function registerView(
    path: string,
    view: EditorView,
    metadata: Partial<EditorViewMetadata> = {}
): void {
    const entry: RegisteredEditorView = {
        view,
        groupIndex: metadata.groupIndex ?? -1,
        readonly: metadata.readonly ?? false
    }
    for (const key of registrationKeys(path)) views.set(key, entry)
    refreshSubscriptions()
}

export function unregisterView(path: string, view?: EditorView): void {
    // When a view is given, only remove the entry if it is still the one
    // registered — a later split group that overwrote the path must not be
    // clobbered by an earlier pane unmounting (m4). No view = unconditional.
    const current = findViewEntry(path)
    if (!current || (view !== undefined && current.view !== view)) return
    for (const [key, entry] of views) {
        if (entry === current) views.delete(key)
    }
    refreshSubscriptions()
}

export function getView(path: string): EditorView | undefined {
    return findViewEntry(path)?.view
}

export function getViewEntry(path: string): RegisteredEditorView | undefined {
    return findViewEntry(path)
}

export type ViewChange = "view" | "document"

interface ViewSubscription {
    path: string
    listener: (change: ViewChange) => void
    view: EditorView | undefined
    pending?: ViewChange
}

const subscriptions = new Set<ViewSubscription>()
const observers = new Map<EditorView, Set<ViewSubscription>>()
// Reuse the empty compartment when a view is subscribed again, rather than
// accumulating an appendConfig slot on every preview toggle.
const compartments = new WeakMap<EditorView, Compartment>()

function notify(subscription: ViewSubscription, change: ViewChange) {
    if (subscription.pending) {
        if (change === "view") subscription.pending = change
        return
    }
    subscription.pending = change
    queueMicrotask(() => {
        const pending = subscription.pending
        subscription.pending = undefined
        if (pending && subscriptions.has(subscription)) subscription.listener(pending)
    })
}

function observe(subscription: ViewSubscription) {
    const view = subscription.view
    if (!view) return
    const existing = observers.get(view)
    if (existing) {
        existing.add(subscription)
        return
    }
    const listeners = new Set([subscription])
    observers.set(view, listeners)
    const compartment = compartments.get(view) ?? new Compartment()
    compartments.set(view, compartment)
    const extension = EditorView.updateListener.of((update) => {
        if (update.docChanged) {
            for (const listener of listeners) notify(listener, "document")
        }
    })
    view.dispatch({
        effects: compartment.get(view.state) === undefined
            ? StateEffect.appendConfig.of(compartment.of(extension))
            : compartment.reconfigure(extension)
    })
}

function unobserve(subscription: ViewSubscription) {
    const view = subscription.view
    if (!view) return
    const listeners = observers.get(view)
    if (!listeners) return
    listeners.delete(subscription)
    if (listeners.size > 0) return
    observers.delete(view)
    view.dispatch({ effects: compartments.get(view)!.reconfigure([]) })
}

function refreshSubscriptions() {
    for (const subscription of subscriptions) {
        const view = getView(subscription.path)
        if (view === subscription.view) continue
        unobserve(subscription)
        subscription.view = view
        observe(subscription)
        notify(subscription, "view")
    }
}

/** Subscribe to subsequent view replacements/removals and immutable doc changes.
 * Notifications run outside CodeMirror's update cycle and coalesce per microtask.
 * Lookup uses the same Windows alias / exact POSIX precedence as getView.
 */
export function subscribeView(path: string, listener: (change: ViewChange) => void): () => void {
    const subscription: ViewSubscription = { path, listener, view: getView(path) }
    subscriptions.add(subscription)
    observe(subscription)
    return () => {
        if (!subscriptions.delete(subscription)) return
        unobserve(subscription)
    }
}

import { useEffectEvent, useLayoutEffect, type RefObject } from "react"

import { PREVIEW_TAB_PATH, type TabInfo } from "@/state/workspaceStore"

// One place decides how every editor-group tab survives being switched away
// from (to an Agent page, another tab, a Space or a workspace) and back.
//
// - "keep-alive": the page stays mounted and is only hidden. Used where the
//   state lives outside React and cannot be snapshotted: Herdr terminals (a
//   live connector stream) and the Browser (the native child webview's page,
//   scroll, form input and in-page app state).
// - "snapshot": the page unmounts and its view state is recorded in the
//   registry below, then replayed when a page for the same document mounts.
//   Cheaper than keeping every editor alive, and a hidden live editor could go
//   stale against an edit made to the same file in another group.
export type TabRetention = "keep-alive" | "snapshot"

function isBrowserTab(tab: Pick<TabInfo, "kind" | "path">): boolean {
    return tab.kind === "preview" || tab.path === PREVIEW_TAB_PATH
}

export function tabRetention(tab: Pick<TabInfo, "kind" | "path">): TabRetention {
    return tab.kind === "herdr-terminal" || isBrowserTab(tab) ? "keep-alive" : "snapshot"
}

// --- View-state registry ----------------------------------------------------
// Keyed by workspace + document path + facet ("editor", "markdown",
// "markdown-preview", ...). Each facet keeps its own recency order and bounds
// (an entry count, and optionally a total weight for heavy values such as
// undo history), so a long session cannot grow it without limit and a heavy
// facet cannot evict the light ones.

export interface RetainedViewState<T> {
    get(): T | undefined
    /** `weight` counts against the facet's weight budget, if it has one. */
    set(value: T, weight?: number): void
    delete(): void
}

interface FacetLimit {
    entries: number
    weight: number
}

interface Retained {
    value: unknown
    weight: number
}

interface Facet {
    entries: Map<string, Retained>
    weight: number
}

const DEFAULT_FACET_LIMIT: FacetLimit = { entries: 200, weight: Infinity }
const facetLimits = new Map<string, FacetLimit>()
const facets = new Map<string, Facet>()

/** Overrides how many documents (default 200) and how much total weight a facet keeps. */
export function setRetainedFacetLimit(facet: string, entries: number, weight = Infinity): void {
    facetLimits.set(facet, { entries, weight })
}

function entryKey(workspacePath: string | null, path: string): string {
    return JSON.stringify([workspacePath, path])
}

function facetOf(name: string): Facet {
    let facet = facets.get(name)
    if (!facet) {
        facet = { entries: new Map(), weight: 0 }
        facets.set(name, facet)
    }
    return facet
}

function forget(facet: Facet, key: string): void {
    const retained = facet.entries.get(key)
    if (!retained) return
    facet.entries.delete(key)
    facet.weight -= retained.weight
}

function remember(name: string, key: string, value: unknown, weight: number): void {
    const facet = facetOf(name)
    forget(facet, key)
    facet.entries.set(key, { value, weight })
    facet.weight += weight
    // Oldest first; an entry heavier than the whole budget evicts itself.
    const limit = facetLimits.get(name) ?? DEFAULT_FACET_LIMIT
    while (facet.entries.size > limit.entries || (facet.entries.size > 0 && facet.weight > limit.weight)) {
        forget(facet, facet.entries.keys().next().value!)
    }
}

export function retainedViewState<T>(
    workspacePath: string | null,
    path: string,
    facet: string
): RetainedViewState<T> {
    const key = entryKey(workspacePath, path)
    return {
        get: () => facets.get(facet)?.entries.get(key)?.value as T | undefined,
        set: (value, weight = 0) => remember(facet, key, value, weight),
        delete: () => {
            const existing = facets.get(facet)
            if (existing) forget(existing, key)
        }
    }
}

/** Moves every facet recorded for a renamed document to its new path. */
export function renameRetainedViewStates(
    workspacePath: string | null,
    fromPath: string,
    toPath: string
): void {
    const fromKey = entryKey(workspacePath, fromPath)
    const toKey = entryKey(workspacePath, toPath)
    for (const [name, facet] of facets) {
        const retained = facet.entries.get(fromKey)
        if (!retained) continue
        forget(facet, fromKey)
        remember(name, toKey, retained.value, retained.weight)
    }
}

export function clearRetainedViewStatesForTest(facet?: string): void {
    if (facet === undefined) facets.clear()
    else facets.delete(facet)
}

// --- Scroll position --------------------------------------------------------

export interface RetainedScrollPosition {
    top: number
    left: number
}

export interface RetainedScrollKey {
    workspacePath: string | null
    path: string
    facet: string
}

// Late images and fonts can grow the content after the first restore; keep
// re-applying the saved offset while it grows, but never fight the user.
const SCROLL_RESTORE_WINDOW_MS = 3000
const USER_INPUT_EVENTS = ["wheel", "pointerdown", "keydown", "touchstart"] as const

/**
 * Restores a scroll container to where its document was last left and records
 * every later scroll. `ready` must turn true in the commit that renders the
 * restorable content (layout effects run after children have filled it).
 * `canRestore` returning false skips or ends the restore, for containers whose
 * position something else owns at that moment (a live scroll sync).
 *
 * The position is recorded while scrolling rather than on unmount: by the time
 * React's cleanup runs the node is detached and reads scrollTop as 0.
 */
export function useRetainedScroll(
    elementRef: RefObject<HTMLElement | null>,
    key: RetainedScrollKey | null,
    ready: boolean,
    canRestore?: () => boolean
): void {
    const workspacePath = key?.workspacePath ?? null
    const path = key?.path ?? null
    const facet = key?.facet ?? null
    const restoreAllowed = useEffectEvent(() => canRestore?.() ?? true)
    useLayoutEffect(() => {
        const element = elementRef.current
        if (!ready || !element || path === null || facet === null) return
        const state = retainedViewState<RetainedScrollPosition>(workspacePath, path, facet)
        const target = state.get()
        let restoring = target !== undefined && (target.top > 0 || target.left > 0) && restoreAllowed()
        let timer: number | undefined

        // Any user input anywhere (the scrollbar sits outside the viewport, and
        // the other group's editor drives a scroll sync) ends the restore.
        const stop = () => {
            if (!restoring) return
            restoring = false
            window.clearTimeout(timer)
            for (const type of USER_INPUT_EVENTS) document.removeEventListener(type, stop, true)
        }
        const apply = () => {
            if (!restoring || !target) return
            if (!restoreAllowed()) {
                stop()
                return
            }
            element.scrollTop = target.top
            element.scrollLeft = target.left
            if (Math.abs(element.scrollTop - target.top) <= 1
                && Math.abs(element.scrollLeft - target.left) <= 1) stop()
        }
        const onScroll = () => {
            // Scroll events caused by the restore itself (possibly clamped while
            // the content is still short) must not overwrite the target.
            if (restoring) return
            state.set({ top: element.scrollTop, left: element.scrollLeft })
        }

        if (restoring) {
            for (const type of USER_INPUT_EVENTS) {
                document.addEventListener(type, stop, { capture: true, passive: true })
            }
            timer = window.setTimeout(stop, SCROLL_RESTORE_WINDOW_MS)
        }
        apply()
        element.addEventListener("scroll", onScroll, { passive: true })
        const observer = restoring && typeof ResizeObserver !== "undefined" ? new ResizeObserver(apply) : null
        observer?.observe(element.firstElementChild ?? element)
        return () => {
            stop()
            observer?.disconnect()
            element.removeEventListener("scroll", onScroll)
        }
    }, [elementRef, workspacePath, path, facet, ready])
}

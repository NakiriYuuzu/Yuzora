import { herdrPagePath } from "@/lib/herdrPages"
import { sameHerdrRuntimeTarget } from "@/lib/herdrRuntime"
import type { HerdrRuntimeTarget, HerdrSnapshot, HerdrTabInfo } from "@/lib/herdrTypes"
import type { TabInfo } from "@/state/workspaceStore"

export interface EditorGroupLike {
    tabs: TabInfo[]
    activePath: string | null
}

export interface HydrateFocusedSpaceHerdrPagesResult<TGroup extends EditorGroupLike = EditorGroupLike> {
    groups: TGroup[]
    activeGroupIndex: number
}

/** Move `from` to `to` with the same splice semantics as TerminalDrawer. */
export function moveItemToIndex<T>(items: readonly T[], from: number, to: number): T[] | null {
    if (from < 0 || from >= items.length) return null
    const clampedTo = Math.max(0, Math.min(items.length - 1, to))
    if (from === clampedTo) return null
    const next = [...items]
    const [moved] = next.splice(from, 1)
    if (moved === undefined) return null
    next.splice(clampedTo, 0, moved)
    return next
}

export function isProjectedHerdrTab(
    tab: TabInfo,
    spaceId: string | null | undefined,
    runtimeWorkspaceId: string | null | undefined = tab.herdrWorkspaceId
): boolean {
    if (tab.kind !== "herdr-terminal") return false
    if (!spaceId) return true
    return (runtimeWorkspaceId ?? null) === spaceId
}

/**
 * Reorder only the visible/projected slots. Hidden tabs keep their exact
 * indices so a drop of A onto B in [A, hidden, B] becomes [B, hidden, A].
 */
export function reorderProjectedSlots<T extends { path: string }>(
    groupTabs: readonly T[],
    projected: readonly T[],
    sourcePath: string,
    destProjectedIndex: number
): T[] | null {
    const sourceProjectedIndex = projected.findIndex((tab) => tab.path === sourcePath)
    if (sourceProjectedIndex < 0) return null
    const nextProjected = moveItemToIndex(projected, sourceProjectedIndex, destProjectedIndex)
    if (!nextProjected) return null
    const projectedPaths = new Set(projected.map((tab) => tab.path))
    const slotIndices: number[] = []
    groupTabs.forEach((tab, index) => {
        if (projectedPaths.has(tab.path)) slotIndices.push(index)
    })
    if (slotIndices.length !== nextProjected.length) return null
    const next = [...groupTabs]
    slotIndices.forEach((index, offset) => {
        next[index] = nextProjected[offset]
    })
    return next
}

export function herdrPageMatchesSnapshotSession(
    tabSessionId: string | undefined,
    snapshotSessionId: string,
    defaultSessionName?: string | null
): boolean {
    if (tabSessionId === "live") {
        return Boolean(defaultSessionName && snapshotSessionId === defaultSessionName)
    }
    return tabSessionId === snapshotSessionId
}

/** Compare both dimensions; missing persisted page target means Native. */
export function herdrPageMatchesSnapshotRuntime(
    tabSessionId: string | undefined,
    tabRuntimeTarget: HerdrRuntimeTarget | null | undefined,
    snapshotSessionId: string,
    snapshotRuntimeTarget: HerdrRuntimeTarget | null | undefined,
    defaultSessionName?: string | null
): boolean {
    return herdrPageMatchesSnapshotSession(tabSessionId, snapshotSessionId, defaultSessionName) &&
        sameHerdrRuntimeTarget(tabRuntimeTarget, snapshotRuntimeTarget)
}

export function resolveSpaceTabCount(
    space: { id: string; tabCount?: number },
    tabs: ReadonlyArray<{ workspaceId: string }> | null | undefined
): number {
    if (tabs && tabs.length > 0) {
        return tabs.reduce((count, tab) => count + (tab.workspaceId === space.id ? 1 : 0), 0)
    }
    return space.tabCount ?? 0
}

/**
 * Destination among same-Space Herdr tabs for protocol `tab.move.insert_index`.
 * Ordinary/hidden tabs only affect the nearest same-Space Herdr slot.
 */
export function herdrInsertIndexForProjectedDrop(
    projected: readonly TabInfo[],
    sourcePath: string,
    destProjectedIndex: number,
    spaceId: string | null | undefined,
    runtimeWorkspaceByTabId?: ReadonlyMap<string, string>
): number | null {
    const isOwnedHerdrTab = (tab: TabInfo) =>
        isProjectedHerdrTab(
            tab,
            spaceId,
            tab.herdrWorkspaceId ??
                (tab.herdrTabId ? runtimeWorkspaceByTabId?.get(tab.herdrTabId) : null)
        )
    const herdrTabs = projected.filter(isOwnedHerdrTab)
    const sourceIndex = herdrTabs.findIndex((tab) => tab.path === sourcePath)
    if (sourceIndex < 0) return null

    const destTab = projected[destProjectedIndex]
    let destIndex: number
    if (destTab && isOwnedHerdrTab(destTab)) {
        destIndex = herdrTabs.findIndex((tab) => tab.path === destTab.path)
    } else {
        destIndex = projected
            .slice(0, Math.max(0, destProjectedIndex))
            .filter(isOwnedHerdrTab).length
        if (destIndex >= herdrTabs.length) destIndex = herdrTabs.length - 1
    }
    if (destIndex < 0) return null
    const clamped = Math.max(0, Math.min(herdrTabs.length - 1, destIndex))
    if (sourceIndex === clamped) return null
    return clamped
}

function usableFocusedSpaceTabs(snapshot: HerdrSnapshot): HerdrTabInfo[] {
    const spaceId = snapshot.focusedWorkspaceId
    if (!spaceId) return []
    return snapshot.tabs.filter(
        (tab) => tab.workspaceId === spaceId && Boolean(tab.terminalId?.trim())
    )
}

function pageMatchesHydrationTab(
    page: TabInfo,
    tab: HerdrTabInfo,
    snapshotSessionId: string,
    snapshotRuntimeTarget: HerdrRuntimeTarget | null | undefined,
    defaultSessionName?: string | null
): boolean {
    if (page.kind !== "herdr-terminal") return false
    if (
        !herdrPageMatchesSnapshotRuntime(
            page.herdrSessionId,
            page.herdrRuntimeTarget,
            snapshotSessionId,
            snapshotRuntimeTarget,
            defaultSessionName
        )
    ) {
        return false
    }
    const pageTabId = page.herdrTabId?.trim() || null
    if (pageTabId) return pageTabId === tab.id
    return Boolean(tab.terminalId && page.terminalId === tab.terminalId)
}

function mergeHydratedPage(page: TabInfo, tab: HerdrTabInfo, snapshot: HerdrSnapshot): TabInfo {
    const paneId =
        tab.paneId ??
        (tab.id === snapshot.focusedTabId
            ? snapshot.focusedPaneId ?? page.paneId ?? null
            : page.paneId ?? null)
    const name = tab.label.trim() || page.name
    if (
        page.name === name &&
        page.kind === "herdr-terminal" &&
        (page.herdrTabId ?? null) === tab.id &&
        (page.herdrWorkspaceId ?? null) === tab.workspaceId &&
        (page.paneId ?? null) === paneId &&
        sameHerdrRuntimeTarget(page.herdrRuntimeTarget, snapshot.runtimeTarget)
    ) {
        return page
    }
    return {
        ...page,
        name,
        kind: "herdr-terminal",
        herdrTabId: tab.id,
        herdrWorkspaceId: tab.workspaceId,
        herdrRuntimeTarget: snapshot.runtimeTarget,
        paneId
    }
}

function createHydratedPage(snapshot: HerdrSnapshot, tab: HerdrTabInfo): TabInfo {
    const terminalId = tab.terminalId!.trim()
    return {
        path: herdrPagePath(snapshot.herdrSessionId, terminalId, snapshot.runtimeTarget),
        name: tab.label.trim() || terminalId,
        dirty: false,
        externallyModified: false,
        kind: "herdr-terminal",
        herdrSessionId: snapshot.herdrSessionId,
        herdrRuntimeTarget: snapshot.runtimeTarget,
        terminalId,
        herdrTabId: tab.id,
        herdrWorkspaceId: tab.workspaceId,
        paneId:
            tab.paneId ??
            (tab.id === snapshot.focusedTabId ? snapshot.focusedPaneId ?? null : null)
    }
}

function isFocusedSpaceHerdrSlot(
    page: TabInfo,
    snapshot: HerdrSnapshot,
    spaceId: string,
    defaultSessionName?: string | null,
    workspaceByTabId?: ReadonlyMap<string, string>
): boolean {
    if (page.kind !== "herdr-terminal") return false
    if (
        !herdrPageMatchesSnapshotRuntime(
            page.herdrSessionId,
            page.herdrRuntimeTarget,
            snapshot.herdrSessionId,
            snapshot.runtimeTarget,
            defaultSessionName
        )
    ) {
        return false
    }
    const pageSpace =
        page.herdrWorkspaceId ??
        (page.herdrTabId ? workspaceByTabId?.get(page.herdrTabId) : undefined)
    return pageSpace === spaceId
}

/**
 * Create missing focused-Space Herdr pages from an authoritative snapshot and
 * permute only those same-Space slots. Ordinary tabs, hidden Spaces, and other
 * named sessions stay put. Never closes existing pages or invents terminal ids.
 */
export function hydrateFocusedSpaceHerdrPages<
    TGroup extends EditorGroupLike
>(
    groups: readonly TGroup[],
    snapshot: HerdrSnapshot,
    defaultSessionName: string | null = null,
    activeGroupIndex = 0
): HydrateFocusedSpaceHerdrPagesResult<TGroup> | null {
    const desired = usableFocusedSpaceTabs(snapshot)
    const spaceId = snapshot.focusedWorkspaceId
    if (!spaceId || desired.length === 0 || groups.length === 0) return null

    const next = groups.map((group) => ({
        ...group,
        tabs: [...group.tabs]
    })) as TGroup[]
    const claimed = new Set<string>()
    type Loc = { groupIndex: number; tabIndex: number }

    const findMatch = (tab: HerdrTabInfo): Loc | null => {
        for (let groupIndex = 0; groupIndex < next.length; groupIndex++) {
            const tabs = next[groupIndex].tabs
            for (let tabIndex = 0; tabIndex < tabs.length; tabIndex++) {
                const page = tabs[tabIndex]
                if (claimed.has(page.path)) continue
                if (
                    !pageMatchesHydrationTab(
                        page,
                        tab,
                        snapshot.herdrSessionId,
                        snapshot.runtimeTarget,
                        defaultSessionName
                    )
                ) {
                    continue
                }
                return { groupIndex, tabIndex }
            }
        }
        return null
    }

    let changed = false
    let focusedPath: string | null = null
    let focusedGroupIndex: number | null = null
    const missing: HerdrTabInfo[] = []

    for (const tab of desired) {
        const loc = findMatch(tab)
        if (!loc) {
            missing.push(tab)
            continue
        }
        const page = next[loc.groupIndex].tabs[loc.tabIndex]
        claimed.add(page.path)
        const merged = mergeHydratedPage(page, tab, snapshot)
        if (merged !== page) {
            next[loc.groupIndex].tabs[loc.tabIndex] = merged
            changed = true
        }
        if (tab.id === snapshot.focusedTabId) {
            focusedPath = merged.path
            focusedGroupIndex = loc.groupIndex
        }
    }

    const insertGroupIndex = next[focusedGroupIndex ?? activeGroupIndex]
        ? (focusedGroupIndex ?? activeGroupIndex)
        : 0
    if (!next[insertGroupIndex]) return null

    for (const tab of missing) {
        const created = createHydratedPage(snapshot, tab)
        next[insertGroupIndex].tabs.push(created)
        changed = true
        if (tab.id === snapshot.focusedTabId) {
            focusedPath = created.path
            focusedGroupIndex = insertGroupIndex
        }
    }

    const workspaceByTabId = new Map(
        snapshot.tabs.map((tab) => [tab.id, tab.workspaceId] as const)
    )
    const desiredIds = desired.map((tab) => tab.id)
    for (const group of next) {
        const indices: number[] = []
        group.tabs.forEach((page, index) => {
            if (
                isFocusedSpaceHerdrSlot(
                    page,
                    snapshot,
                    spaceId,
                    defaultSessionName,
                    workspaceByTabId
                )
            ) {
                indices.push(index)
            }
        })
        if (indices.length === 0) continue
        const current = indices.map((index) => group.tabs[index])
        const leftover = new Map<string, TabInfo>()
        const unmatched: TabInfo[] = []
        for (const page of current) {
            if (page.herdrTabId && !leftover.has(page.herdrTabId)) {
                leftover.set(page.herdrTabId, page)
            } else {
                unmatched.push(page)
            }
        }
        const ordered: TabInfo[] = []
        for (const id of desiredIds) {
            const page = leftover.get(id)
            if (!page) continue
            ordered.push(page)
            leftover.delete(id)
        }
        for (const page of current) {
            if (page.herdrTabId && leftover.has(page.herdrTabId)) {
                ordered.push(page)
                leftover.delete(page.herdrTabId)
            }
        }
        for (const page of unmatched) ordered.push(page)
        if (ordered.length !== indices.length) continue
        if (ordered.every((page, offset) => page === current[offset])) continue
        changed = true
        indices.forEach((index, offset) => {
            group.tabs[index] = ordered[offset]
        })
    }

    const nextActiveGroupIndex = focusedGroupIndex ?? activeGroupIndex
    if (focusedPath && next[nextActiveGroupIndex]?.activePath !== focusedPath) {
        next[nextActiveGroupIndex] = {
            ...next[nextActiveGroupIndex],
            activePath: focusedPath
        }
        changed = true
    }

    if (!changed && nextActiveGroupIndex === activeGroupIndex) return null
    return {
        groups: changed ? next : (groups as TGroup[]),
        activeGroupIndex: nextActiveGroupIndex
    }
}

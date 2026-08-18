import { create } from "zustand"
import type { DocumentLineEnding } from "../lib/types"
import type { HerdrRuntimeTarget, HerdrSnapshot } from "../lib/herdrTypes"
import { herdrPagePath, isHerdrPagePath } from "../lib/herdrPages"
import {
    normalizeHerdrRuntimeTarget,
    sameHerdrRuntimeTarget
} from "../lib/herdrRuntime"
import {
    isFileTab,
    isMarkdownPreviewForSource,
    isMarkdownPreviewPath,
    isMarkdownPreviewTab,
    markdownPreviewPath,
    markdownPreviewSourcePath,
    markdownPreviewTabName,
    previewTabSourcePath
} from "../lib/markdownPreviewTab"
import { rebasePath, workspacePathBasename } from "../lib/paths"
import {
    herdrPageMatchesSnapshotRuntime,
    hydrateFocusedSpaceHerdrPages,
    moveItemToIndex,
    reorderProjectedSlots
} from "../lib/workbenchTabReorder"

// Singleton preview tab: a reserved sentinel path keeps preview inside the
// path-keyed tab model without ever being mistaken for a real file. Only one
// preview tab exists app-wide (see openPreviewTab).
export const PREVIEW_TAB_PATH = "yuzora://preview"
const PREVIEW_TAB_NAME = "Preview"

export type TabKind = "file" | "preview" | "markdown-preview" | "herdr-terminal"

export interface TabInfo {
    path: string
    name: string
    dirty: boolean
    externallyModified: boolean
    lineEnding?: DocumentLineEnding
    lineEndingGeneration?: number
    // Absent ⇒ a normal file tab. "preview" marks the singleton preview tab so
    // EditorArea/TabBar can special-case it without touching file-path logic.
    // "herdr-terminal" pages use path = herdrPagePath(sessionId, terminalId);
    // paneId / herdrTabId are mutable metadata. When herdrTabId is present,
    // open pages dedupe by (sessionName, tabId) instead of terminalId.
    kind?: TabKind
    /** Only for kind === "markdown-preview"; canonical source file path. */
    sourcePath?: string
    herdrSessionId?: string
    /** Missing persisted value is a legacy Native page. */
    herdrRuntimeTarget?: HerdrRuntimeTarget | null
    terminalId?: string
    herdrTabId?: string | null
    /** Owning Herdr Space/workspace identity for selected-Space tab projection. */
    herdrWorkspaceId?: string | null
    paneId?: string | null
}

export interface EditorGroup {
    /** Runtime-stable identity for React keys. Optional on legacy/test state. */
    id?: string
    tabs: TabInfo[]
    activePath: string | null
}

interface PendingReveal {
    path: string
    line: number
    // Whether revealing should also steal editor focus. Omitted (undefined) means
    // the default — navigations (go-to-definition, symbol jump) focus the editor;
    // search-result clicks pass false to reveal-only, preserving M2 behaviour (A4).
    // Consumers apply `?? true`.
    focus?: boolean
}

interface WorkspaceState {
    workspacePath: string | null
    workspaceCapabilityId: string | null
    groups: EditorGroup[]
    activeGroupIndex: number
    pendingReveal: PendingReveal | null
    // Bumped after a file-tree mutation (new/rename/delete via the context menu)
    // to force FileTree to re-list. The FileTree doesn't subscribe to the fs
    // watcher, so these in-app operations need an explicit refresh signal.
    treeRevision: number
    /** Cold-start file-session restore has settled; Herdr focus may now restore its view. */
    sessionRestoreReady: boolean
    markSessionRestoreReady: () => void
    setWorkspace: (path: string, capabilityId?: string | null) => void
    openTab: (path: string, groupIndex?: number) => void
    /** Open or move a file tab into one exact Editor Group. */
    openTabInGroup: (path: string, groupIndex: number) => void
    openInRightSplit: (path: string, sourceGroupIndex: number) => void
    splitAndMoveRight: (groupIndex: number, path: string) => void
    refreshTree: () => void
    closeTab: (groupIndex: number, path: string) => void
    closeOtherTabs: (groupIndex: number, keepPath: string) => void
    closeAllTabs: (groupIndex: number) => void
    closeTabsByPath: (paths: string[]) => void
    /** Same-group identity move. Invalid path / no-op destination leave state unchanged. */
    reorderTab: (groupIndex: number, path: string, destinationIndex: number) => void
    /** Reorder only visible/projected slots, preserving hidden-Space pages. */
    reorderProjectedTab: (
        groupIndex: number,
        path: string,
        destProjectedIndex: number,
        projected: TabInfo[]
    ) => void
    /** Reorder only Herdr pages in each Space to match an authoritative snapshot. */
    reconcileHerdrPagesFromSnapshot: (
        snapshot: HerdrSnapshot,
        defaultSessionName?: string | null
    ) => void
    /** Create missing focused-Space Herdr pages from a snapshot without stealing final focus. */
    hydrateHerdrPagesFromSnapshot: (
        snapshot: HerdrSnapshot,
        defaultSessionName?: string | null
    ) => void
    updateTabPath: (fromPath: string, toPath: string) => void
    openPreviewTab: (groupIndex?: number) => void
    closePreviewTab: () => void
    togglePreviewTab: () => void
    toggleMarkdownPreview: (sourcePath: string, sourceGroupIndex: number) => void
    openMarkdownPreviewInAdjacentGroup: (sourcePath: string, sourceGroupIndex: number) => void
    closeMarkdownPreviewTab: (groupIndex: number, previewPath: string) => void
    closeMarkdownPreviewsForSource: (sourcePath: string) => void
    hasMarkdownPreview: (sourcePath: string) => boolean
    /** Open or focus a Herdr terminal page. Dedupes by (session, tabId) when available. */
    openHerdrTerminalPage: (args: {
        herdrSessionId: string
        runtimeTarget?: HerdrRuntimeTarget | null
        terminalId: string
        title?: string
        paneId?: string | null
        herdrTabId?: string | null
        herdrWorkspaceId?: string | null
        groupIndex?: number
    }) => void
    /** paneId is mutable topology metadata — update without recreating the page. */
    updateHerdrPagePaneId: (pagePath: string, paneId: string | null) => void
    /** Resolve a legacy terminal page to its owning Herdr tab identity. */
    updateHerdrPageTabId: (pagePath: string, herdrTabId: string) => void
    /** Keep the Yuzora page label synchronized after an explicit Herdr tab rename. */
    updateHerdrPageTitle: (pagePath: string, title: string) => void
    setActiveTab: (groupIndex: number, path: string) => void
    setActiveGroup: (groupIndex: number) => void
    markDirty: (path: string, dirty: boolean) => void
    hydrateLineEnding: (
        path: string,
        lineEnding: DocumentLineEnding | undefined,
        generation: number
    ) => void
    setLineEnding: (path: string, lineEnding: Exclude<DocumentLineEnding, "mixed">) => void
    getLineEnding: (path: string) => DocumentLineEnding | undefined
    markExternallyModified: (path: string, modified: boolean) => void
    splitRight: () => void
    closeSplit: () => void
    requestReveal: (
        path: string,
        line: number,
        focus?: boolean,
        groupIndex?: number
    ) => void
    consumeReveal: () => void
}

let nextEditorGroupSeq = 0

function nextEditorGroupId(): string {
    nextEditorGroupSeq += 1
    return `editor-group-${nextEditorGroupSeq}`
}

const emptyGroup = (): EditorGroup => ({
    id: nextEditorGroupId(),
    tabs: [],
    activePath: null
})

function mergeConservativeTab(primary: TabInfo, other: TabInfo): TabInfo {
    const otherHasNewerLineEnding =
        other.lineEndingGeneration !== undefined &&
        (primary.lineEndingGeneration === undefined ||
            other.lineEndingGeneration > primary.lineEndingGeneration)
    const lineEnding = otherHasNewerLineEnding
        ? other.lineEnding
        : primary.lineEndingGeneration !== undefined
          ? primary.lineEnding
          : primary.lineEnding ?? other.lineEnding
    const lineEndingGeneration = otherHasNewerLineEnding
        ? other.lineEndingGeneration
        : primary.lineEndingGeneration ?? other.lineEndingGeneration
    return {
        ...primary,
        dirty: primary.dirty || other.dirty,
        externallyModified: primary.externallyModified || other.externallyModified,
        lineEnding,
        lineEndingGeneration,
        kind: primary.kind ?? other.kind,
        sourcePath: primary.sourcePath ?? other.sourcePath
    }
}

function makeMarkdownPreviewTab(sourcePath: string): TabInfo {
    return {
        path: markdownPreviewPath(sourcePath),
        name: markdownPreviewTabName(),
        dirty: false,
        externallyModified: false,
        kind: "markdown-preview",
        sourcePath
    }
}

function findMarkdownPreview(
    groups: EditorGroup[],
    sourcePath: string
): { groupIndex: number; tab: TabInfo } | null {
    for (const [groupIndex, group] of groups.entries()) {
        const tab = group.tabs.find((candidate) => isMarkdownPreviewForSource(candidate, sourcePath))
        if (tab) return { groupIndex, tab }
    }
    return null
}

function adjacentMarkdownPreviewGroupIndex(sourceGroupIndex: number): number {
    return sourceGroupIndex === 0 ? 1 : 0
}

function withoutMarkdownPreviewsForSource(group: EditorGroup, sourcePath: string): EditorGroup {
    const tabs = group.tabs.filter((tab) => !isMarkdownPreviewForSource(tab, sourcePath))
    if (tabs.length === group.tabs.length) return group
    return {
        ...group,
        tabs,
        activePath:
            group.activePath && tabs.some((tab) => tab.path === group.activePath)
                ? group.activePath
                : tabs.at(-1)?.path ?? null
    }
}

function removeGroupAt(
    groups: EditorGroup[],
    groupIndex: number,
    activeGroupIndex: number
): { groups: EditorGroup[]; activeGroupIndex: number } {
    if (!groups[groupIndex]) return { groups, activeGroupIndex }
    if (groups.length <= 1) {
        return { groups: [emptyGroup()], activeGroupIndex: 0 }
    }
    const next = groups.filter((_, index) => index !== groupIndex)
    let active = activeGroupIndex
    if (groupIndex === 0 || active === groupIndex) active = 0
    else if (active > groupIndex) active -= 1
    return {
        groups: next.length > 0 ? next : [emptyGroup()],
        activeGroupIndex: Math.max(0, Math.min(active, Math.max(next.length - 1, 0)))
    }
}

function pruneEmptiedGroups(
    groups: EditorGroup[],
    emptiedIndexes: number[],
    activeGroupIndex: number
): { groups: EditorGroup[]; activeGroupIndex: number } {
    let nextGroups = groups
    let nextActive = activeGroupIndex
    for (const index of [...emptiedIndexes].sort((left, right) => right - left)) {
        if (nextGroups[index]?.tabs.length !== 0) continue
        const pruned = removeGroupAt(nextGroups, index, nextActive)
        nextGroups = pruned.groups
        nextActive = pruned.activeGroupIndex
    }
    return { groups: nextGroups, activeGroupIndex: nextActive }
}

function closeMarkdownPreviewsInGroups(
    groups: EditorGroup[],
    sourcePath: string,
    activeGroupIndex: number
): { groups: EditorGroup[]; activeGroupIndex: number } {
    const emptied: number[] = []
    const next = groups.map((group, index) => {
        const updated = withoutMarkdownPreviewsForSource(group, sourcePath)
        if (updated !== group && updated.tabs.length === 0) emptied.push(index)
        return updated
    })
    return pruneEmptiedGroups(next, emptied, activeGroupIndex)
}

function collectMarkdownPreviewsForSource(groups: EditorGroup[], sourcePath: string): TabInfo[] {
    const found: TabInfo[] = []
    for (const group of groups) {
        for (const tab of group.tabs) {
            if (isMarkdownPreviewForSource(tab, sourcePath)) found.push(tab)
        }
    }
    return found
}

function rehomeMarkdownPreviewForMovedSource(
    groups: EditorGroup[],
    sourcePath: string,
    sourceGroupIndex: number,
    activeGroupIndex: number
): { groups: EditorGroup[]; activeGroupIndex: number } {
    if (!groups[sourceGroupIndex]?.tabs.some((tab) => tab.path === sourcePath && isFileTab(tab))) {
        return { groups, activeGroupIndex }
    }
    const matches = collectMarkdownPreviewsForSource(groups, sourcePath)
    if (matches.length === 0) return { groups, activeGroupIndex }

    let existing: TabInfo | undefined
    for (const tab of matches) {
        existing = existing ? mergeConservativeTab(existing, tab) : tab
    }

    const emptied: number[] = []
    const stripped = groups.map((group, index) => {
        const updated = withoutMarkdownPreviewsForSource(group, sourcePath)
        if (updated !== group && updated.tabs.length === 0) emptied.push(index)
        return updated
    })

    let destinationIndex = adjacentMarkdownPreviewGroupIndex(sourceGroupIndex)
    let next = stripped
    if (destinationIndex >= next.length) {
        if (sourceGroupIndex !== 0) {
            return pruneEmptiedGroups(next, emptied, activeGroupIndex)
        }
        next = [...next, emptyGroup()]
        destinationIndex = next.length - 1
    }

    const preview = {
        ...existing!,
        path: markdownPreviewPath(sourcePath),
        name: markdownPreviewTabName(),
        sourcePath,
        kind: "markdown-preview" as const
    }
    next = next.map((group, index) => {
        if (index !== destinationIndex) return group
        return {
            ...group,
            tabs: [...group.tabs, preview],
            activePath: preview.path
        }
    })
    return pruneEmptiedGroups(
        next,
        emptied.filter((index) => index !== destinationIndex),
        activeGroupIndex
    )
}

function withoutTab(group: EditorGroup, path: string): EditorGroup {
    const tabs = group.tabs.filter((tab) => tab.path !== path)
    return {
        ...group,
        tabs,
        activePath:
            group.activePath === path
                ? tabs.at(-1)?.path ?? null
                : group.activePath
    }
}

export const useWorkspaceStore = create<WorkspaceState>((set, get) => ({
    workspacePath: null,
    workspaceCapabilityId: null,
    groups: [emptyGroup()],
    activeGroupIndex: 0,
    pendingReveal: null,
    treeRevision: 0,
    sessionRestoreReady: false,
    markSessionRestoreReady: () => set({ sessionRestoreReady: true }),
    setWorkspace: (path, capabilityId = null) =>
        set((state) => {
            // Herdr pages represent runtime tabs, not files owned by the current
            // workspace. Preserve their exact editor-group placement so React
            // keeps the existing xterm/connector instances mounted while file
            // and preview pages are replaced.
            const groups = state.groups.map((group) => {
                const tabs = group.tabs.filter((tab) => tab.kind === "herdr-terminal")
                const activePath =
                    group.activePath && tabs.some((tab) => tab.path === group.activePath)
                        ? group.activePath
                        : tabs.at(-1)?.path ?? null
                return { ...group, tabs, activePath }
            })
            const activeGroupIndex = groups[state.activeGroupIndex]
                ? state.activeGroupIndex
                : 0
            return {
                workspacePath: path,
                workspaceCapabilityId: capabilityId,
                groups,
                activeGroupIndex,
                pendingReveal: null
            }
        }),
    refreshTree: () => set((s) => ({ treeRevision: s.treeRevision + 1 })),
    openTab: (path, groupIndex) =>
        set((s) => {
            const existingGroupIndex = s.groups.findIndex((group) =>
                group.tabs.some((tab) => tab.path === path)
            )
            if (existingGroupIndex !== -1) {
                return {
                    groups: s.groups.map((group, index) =>
                        index === existingGroupIndex ? { ...group, activePath: path } : group
                    ),
                    activeGroupIndex: existingGroupIndex
                }
            }
            const targetGroupIndex = groupIndex ?? s.activeGroupIndex
            if (!s.groups[targetGroupIndex]) return s
            const groups = s.groups.map((g) => ({ ...g, tabs: [...g.tabs] }))
            const g = groups[targetGroupIndex]
            if (!g.tabs.some((t) => t.path === path)) {
                g.tabs.push({
                    path,
                    name: workspacePathBasename(path),
                    dirty: false,
                    externallyModified: false
                })
            }
            g.activePath = path
            return { groups, activeGroupIndex: targetGroupIndex }
        }),
    openTabInGroup: (path, groupIndex) =>
        set((s) => {
            if (isMarkdownPreviewPath(path) || path === PREVIEW_TAB_PATH) return s
            if (!s.groups[groupIndex]) return s

            let tab: TabInfo | undefined
            let targetPosition = -1
            for (const [index, group] of s.groups.entries()) {
                const position = group.tabs.findIndex((candidate) => candidate.path === path)
                if (position === -1) continue
                if (index === groupIndex) targetPosition = position
                tab = tab
                    ? mergeConservativeTab(tab, group.tabs[position])
                    : group.tabs[position]
            }
            tab ??= {
                path,
                name: workspacePathBasename(path),
                dirty: false,
                externallyModified: false
            }

            const groups = s.groups.map((group) => withoutTab(group, path))
            const target = groups[groupIndex]
            const insertAt = targetPosition === -1
                ? target.tabs.length
                : Math.min(targetPosition, target.tabs.length)
            target.tabs.splice(insertAt, 0, tab)
            target.activePath = path
            if (!isFileTab(tab)) return { groups, activeGroupIndex: groupIndex }
            return rehomeMarkdownPreviewForMovedSource(groups, path, groupIndex, groupIndex)
        }),
    openInRightSplit: (path, sourceGroupIndex) =>
        set((s) => {
            if (isMarkdownPreviewPath(path) || path === PREVIEW_TAB_PATH) return s
            if (!s.groups[sourceGroupIndex]) return s
            if (s.groups.length >= 2 && sourceGroupIndex >= s.groups.length - 1) return s

            const groups = s.groups.map((group) => ({ ...group, tabs: [...group.tabs] }))
            const destinationIndex = sourceGroupIndex + 1
            if (groups.length <= destinationIndex) groups.push(emptyGroup())
            if (destinationIndex >= 2) return s

            const destinationPosition = groups[destinationIndex].tabs.findIndex(
                (tab) => tab.path === path
            )
            let tab: TabInfo | undefined
            for (const group of groups) {
                for (const candidate of group.tabs) {
                    if (candidate.path !== path) continue
                    tab = tab ? mergeConservativeTab(tab, candidate) : candidate
                }
            }
            tab ??= {
                path,
                name: workspacePathBasename(path),
                dirty: false,
                externallyModified: false
            }

            const withoutTarget = groups.map((group) => withoutTab(group, path))
            const destination = withoutTarget[destinationIndex]
            const insertAt = destinationPosition === -1
                ? destination.tabs.length
                : Math.min(destinationPosition, destination.tabs.length)
            destination.tabs.splice(insertAt, 0, tab)
            destination.activePath = path
            if (!isFileTab(tab)) {
                return { groups: withoutTarget, activeGroupIndex: destinationIndex }
            }
            return rehomeMarkdownPreviewForMovedSource(
                withoutTarget,
                path,
                destinationIndex,
                destinationIndex
            )
        }),
    splitAndMoveRight: (groupIndex, path) =>
        set((s) => {
            const source = s.groups[groupIndex]
            const clicked = source?.tabs.find((tab) => tab.path === path)
            if (!clicked || clicked.kind === "preview" || isMarkdownPreviewTab(clicked)) return s
            if (s.groups.length >= 2 && groupIndex >= s.groups.length - 1) return s

            const groups = s.groups.map((group) => ({ ...group, tabs: [...group.tabs] }))
            const destinationIndex = groupIndex + 1
            if (groups.length <= destinationIndex) groups.push(emptyGroup())
            if (destinationIndex >= 2) return s

            const destinationDuplicate = groups[destinationIndex].tabs.find(
                (tab) => tab.path === path
            )
            const destinationPosition = groups[destinationIndex].tabs.findIndex(
                (tab) => tab.path === path
            )
            const moved = destinationDuplicate
                ? mergeConservativeTab(clicked, destinationDuplicate)
                : clicked
            const withoutTarget = groups.map((group) => withoutTab(group, path))
            const destination = withoutTarget[destinationIndex]
            const insertAt = destinationPosition === -1
                ? destination.tabs.length
                : Math.min(destinationPosition, destination.tabs.length)
            destination.tabs.splice(insertAt, 0, moved)
            destination.activePath = path
            return rehomeMarkdownPreviewForMovedSource(
                withoutTarget,
                path,
                destinationIndex,
                destinationIndex
            )
        }),
    closeTab: (groupIndex, path) =>
        set((s) => {
            const groups = s.groups.map((g) => ({ ...g, tabs: [...g.tabs] }))
            const g = groups[groupIndex]
            if (!g) return s
            const tab = g.tabs.find((candidate) => candidate.path === path)
            g.tabs = g.tabs.filter((t) => t.path !== path)
            if (g.activePath === path) g.activePath = g.tabs.at(-1)?.path ?? null
            if (tab && isMarkdownPreviewTab(tab)) {
                return pruneEmptiedGroups(
                    groups,
                    g.tabs.length === 0 ? [groupIndex] : [],
                    s.activeGroupIndex
                )
            }
            if (tab && isFileTab(tab)) {
                return closeMarkdownPreviewsInGroups(groups, tab.path, s.activeGroupIndex)
            }
            return { groups }
        }),
    // Pure tab-list mutations for the tab context menu's "Close others" /
    // "Close all" — the confirm-dialog + document/preview cleanup side effects
    // (mirroring TabBar's onClose) live in contextMenuStore, which calls these
    // after resolving any dirty-tab confirmation.
    closeOtherTabs: (groupIndex, keepPath) =>
        set((s) => {
            const group = s.groups[groupIndex]
            if (!group) return s
            const closedSources = group.tabs
                .filter((tab) => tab.path !== keepPath && isFileTab(tab))
                .map((tab) => tab.path)
            const groups = s.groups.map((candidate, index) => {
                if (index !== groupIndex) return candidate
                const tabs = candidate.tabs.filter((tab) => tab.path === keepPath)
                return { ...candidate, tabs, activePath: tabs.length > 0 ? keepPath : null }
            })
            let next = { groups, activeGroupIndex: s.activeGroupIndex }
            for (const source of closedSources) {
                next = closeMarkdownPreviewsInGroups(next.groups, source, next.activeGroupIndex)
            }
            return next
        }),
    closeAllTabs: (groupIndex) =>
        set((s) => {
            const group = s.groups[groupIndex]
            if (!group) return s
            const closedSources = group.tabs.filter(isFileTab).map((tab) => tab.path)
            const groups = s.groups.map((candidate, index) =>
                index === groupIndex ? { ...candidate, tabs: [], activePath: null } : candidate
            )
            let next = { groups, activeGroupIndex: s.activeGroupIndex }
            for (const source of closedSources) {
                next = closeMarkdownPreviewsInGroups(next.groups, source, next.activeGroupIndex)
            }
            return next
        }),
    // Bulk close every tab (across ALL groups) whose path is in `paths`. Used
    // after a file/folder delete: a tab left pointing at a now-gone path would
    // let its EditorPane recreate the file on the next save. activePath falls
    // back to the last surviving tab, mirroring closeTab's rule.
    closeTabsByPath: (paths) =>
        set((s) => {
            const drop = new Set(paths)
            const closedSources = new Set<string>()
            for (const group of s.groups) {
                for (const tab of group.tabs) {
                    if (drop.has(tab.path) && isFileTab(tab)) closedSources.add(tab.path)
                }
            }
            const emptied: number[] = []
            const groups = s.groups.map((group, index) => {
                const tabs = group.tabs.filter((tab) => {
                    if (drop.has(tab.path)) return false
                    if (isMarkdownPreviewTab(tab)) {
                        const source = previewTabSourcePath(tab)
                        if (source && closedSources.has(source)) return false
                    }
                    return true
                })
                if (tabs.length === group.tabs.length) return group
                const emptiedByPreviewCleanup =
                    tabs.length === 0 &&
                    group.tabs.every((tab) => isMarkdownPreviewTab(tab) || drop.has(tab.path)) &&
                    !group.tabs.some((tab) => !isMarkdownPreviewTab(tab) && drop.has(tab.path))
                if (emptiedByPreviewCleanup) emptied.push(index)
                const activePath =
                    group.activePath !== null && !tabs.some((tab) => tab.path === group.activePath)
                        ? tabs.at(-1)?.path ?? null
                        : group.activePath
                return { ...group, tabs, activePath }
            })
            return pruneEmptiedGroups(groups, emptied, s.activeGroupIndex)
        }),
    reorderTab: (groupIndex, path, destinationIndex) =>
        set((s) => {
            const group = s.groups[groupIndex]
            if (!group) return s
            const sourceIndex = group.tabs.findIndex((tab) => tab.path === path)
            const tabs = moveItemToIndex(group.tabs, sourceIndex, destinationIndex)
            if (!tabs) return s
            return {
                groups: s.groups.map((candidate, index) =>
                    index === groupIndex
                        ? { ...candidate, tabs, activePath: candidate.activePath }
                        : candidate
                )
            }
        }),
    reorderProjectedTab: (groupIndex, path, destProjectedIndex, projected) =>
        set((s) => {
            const group = s.groups[groupIndex]
            if (!group) return s
            const tabs = reorderProjectedSlots(group.tabs, projected, path, destProjectedIndex)
            if (!tabs) return s
            return {
                groups: s.groups.map((candidate, index) =>
                    index === groupIndex
                        ? { ...candidate, tabs, activePath: candidate.activePath }
                        : candidate
                )
            }
        }),
    reconcileHerdrPagesFromSnapshot: (snapshot, defaultSessionName = null) =>
        set((s) => {
            const orderBySpace = new Map<string, string[]>()
            const workspaceByTabId = new Map<string, string>()
            for (const tab of snapshot.tabs) {
                const ids = orderBySpace.get(tab.workspaceId) ?? []
                ids.push(tab.id)
                orderBySpace.set(tab.workspaceId, ids)
                workspaceByTabId.set(tab.id, tab.workspaceId)
            }
            let changed = false
            const groups = s.groups.map((group) => {
                const next = [...group.tabs]
                const indicesBySpace = new Map<string, number[]>()
                next.forEach((tab, index) => {
                    if (tab.kind !== "herdr-terminal" || !tab.herdrTabId) return
                    if (
                        tab.herdrRuntimeTarget === undefined &&
                        sameHerdrRuntimeTarget(tab.herdrRuntimeTarget, snapshot.runtimeTarget)
                    ) {
                        next[index] = { ...tab, herdrRuntimeTarget: normalizeHerdrRuntimeTarget(snapshot.runtimeTarget) }
                        tab = next[index]
                        changed = true
                    }
                    if (
                        !herdrPageMatchesSnapshotRuntime(
                            tab.herdrSessionId,
                            tab.herdrRuntimeTarget,
                            snapshot.herdrSessionId,
                            snapshot.runtimeTarget,
                            defaultSessionName
                        )
                    ) {
                        return
                    }
                    const spaceId =
                        tab.herdrWorkspaceId ?? workspaceByTabId.get(tab.herdrTabId!)
                    if (!spaceId) return
                    const indices = indicesBySpace.get(spaceId) ?? []
                    indices.push(index)
                    indicesBySpace.set(spaceId, indices)
                })
                for (const [spaceId, indices] of indicesBySpace) {
                    const desired = orderBySpace.get(spaceId)
                    if (!desired || indices.length === 0) continue
                    const current = indices.map((index) => next[index])
                    const leftover = new Map(
                        current
                            .filter((tab) => tab.herdrTabId)
                            .map((tab) => [tab.herdrTabId!, tab])
                    )
                    const ordered: TabInfo[] = []
                    for (const id of desired) {
                        const tab = leftover.get(id)
                        if (!tab) continue
                        ordered.push(tab)
                        leftover.delete(id)
                    }
                    for (const tab of current) {
                        if (tab.herdrTabId && leftover.has(tab.herdrTabId)) {
                            ordered.push(tab)
                            leftover.delete(tab.herdrTabId)
                        }
                    }
                    if (ordered.length !== indices.length) continue
                    const same = ordered.every((tab, offset) => tab === current[offset])
                    if (same) continue
                    changed = true
                    indices.forEach((index, offset) => {
                        next[index] = ordered[offset]
                    })
                }
                return changed ? { ...group, tabs: next } : group
            })
            return changed ? { groups } : s
        }),
    hydrateHerdrPagesFromSnapshot: (snapshot, defaultSessionName = null) =>
        set((s) => {
            const result = hydrateFocusedSpaceHerdrPages(
                s.groups,
                snapshot,
                defaultSessionName,
                s.activeGroupIndex
            )
            if (!result) return s
            const groupsChanged = result.groups.some((group, index) => group !== s.groups[index])
            if (!groupsChanged && result.activeGroupIndex === s.activeGroupIndex) return s
            return {
                groups: groupsChanged ? result.groups : s.groups,
                activeGroupIndex: result.activeGroupIndex
            }
        }),
    // Re-point tabs (across ALL groups) after a file/folder rename: a tab at
    // exactly `fromPath` moves to `toPath`; a tab under `fromPath/` (folder
    // rename) has its prefix rewritten. path + name (+ any matching activePath)
    // are updated in place, preserving the tab's dirty flag and position.
    updateTabPath: (fromPath, toPath) =>
        set((s) => {
            const remap = (p: string): string | null => rebasePath(fromPath, toPath, p)
            return {
                groups: s.groups.map((g) => {
                    let changed = false
                    const tabs = g.tabs.map((t) => {
                        if (isMarkdownPreviewTab(t)) {
                            const oldSource = previewTabSourcePath(t)
                            if (!oldSource) return t
                            const newSource = remap(oldSource)
                            if (!newSource) return t
                            changed = true
                            return {
                                ...t,
                                path: markdownPreviewPath(newSource),
                                name: markdownPreviewTabName(),
                                sourcePath: newSource,
                                kind: "markdown-preview" as const
                            }
                        }
                        const np = remap(t.path)
                        if (np === null) return t
                        changed = true
                        return { ...t, path: np, name: workspacePathBasename(np) }
                    })
                    let activePath = g.activePath
                    if (activePath !== null) {
                        const remappedActive = remap(activePath)
                        if (remappedActive !== null) {
                            activePath = remappedActive
                        } else {
                            const source = markdownPreviewSourcePath(activePath)
                            if (source) {
                                const newSource = remap(source)
                                if (newSource) activePath = markdownPreviewPath(newSource)
                            }
                        }
                    }
                    if (!changed && activePath === g.activePath) return g
                    return { ...g, tabs, activePath }
                })
            }
        }),
    openPreviewTab: (groupIndex) =>
        set((s) => {
            // Singleton: if a preview tab already exists in any group, just
            // focus it (and its group) rather than opening a second one.
            const existing = s.groups.findIndex((g) =>
                g.tabs.some((t) => t.path === PREVIEW_TAB_PATH)
            )
            if (existing !== -1) {
                return {
                    groups: s.groups.map((g, i) =>
                        i === existing ? { ...g, activePath: PREVIEW_TAB_PATH } : g
                    ),
                    activeGroupIndex: existing
                }
            }
            const targetGroupIndex = groupIndex ?? s.activeGroupIndex
            if (!s.groups[targetGroupIndex]) return s
            const groups = s.groups.map((g) => ({ ...g, tabs: [...g.tabs] }))
            const g = groups[targetGroupIndex]
            g.tabs.push({
                path: PREVIEW_TAB_PATH,
                name: PREVIEW_TAB_NAME,
                dirty: false,
                externallyModified: false,
                kind: "preview"
            })
            g.activePath = PREVIEW_TAB_PATH
            return { groups, activeGroupIndex: targetGroupIndex }
        }),
    closePreviewTab: () =>
        set((s) => ({
            groups: s.groups.map((g) => {
                if (!g.tabs.some((t) => t.path === PREVIEW_TAB_PATH)) return g
                const tabs = g.tabs.filter((t) => t.path !== PREVIEW_TAB_PATH)
                return {
                    ...g,
                    tabs,
                    activePath:
                        g.activePath === PREVIEW_TAB_PATH
                            ? tabs.at(-1)?.path ?? null
                            : g.activePath
                }
            })
        })),
    togglePreviewTab: () => {
        // Focused preview ⇒ close; otherwise open-or-focus (rail toggle semantics).
        const s = get()
        if (s.groups[s.activeGroupIndex]?.activePath === PREVIEW_TAB_PATH) {
            get().closePreviewTab()
        } else {
            get().openPreviewTab()
        }
    },
    toggleMarkdownPreview: (sourcePath, sourceGroupIndex) => {
        if (collectMarkdownPreviewsForSource(get().groups, sourcePath).length > 0) {
            set((s) => closeMarkdownPreviewsInGroups(s.groups, sourcePath, s.activeGroupIndex))
            return
        }
        get().openMarkdownPreviewInAdjacentGroup(sourcePath, sourceGroupIndex)
    },
    openMarkdownPreviewInAdjacentGroup: (sourcePath, sourceGroupIndex) =>
        set((s) => {
            const sourceGroup = s.groups[sourceGroupIndex]
            if (!sourceGroup?.tabs.some((tab) => tab.path === sourcePath && isFileTab(tab))) {
                return s
            }
            if (s.groups.length < 2 && sourceGroupIndex !== 0) return s

            const groups = s.groups.map((group) => ({ ...group, tabs: [...group.tabs] }))
            if (groups.length < 2) groups.push(emptyGroup())
            const destinationIndex = adjacentMarkdownPreviewGroupIndex(sourceGroupIndex)
            if (!groups[destinationIndex]) return s

            let existing: TabInfo | undefined
            for (const group of groups) {
                for (const tab of group.tabs) {
                    if (!isMarkdownPreviewForSource(tab, sourcePath)) continue
                    existing = existing ? mergeConservativeTab(existing, tab) : tab
                }
            }
            const withoutExisting = groups.map((group) =>
                withoutMarkdownPreviewsForSource(group, sourcePath)
            )
            const preview = existing
                ? {
                      ...existing,
                      path: markdownPreviewPath(sourcePath),
                      name: markdownPreviewTabName(),
                      sourcePath,
                      kind: "markdown-preview" as const
                  }
                : makeMarkdownPreviewTab(sourcePath)
            const destination = withoutExisting[destinationIndex]
            destination.tabs.push(preview)
            destination.activePath = preview.path
            return { groups: withoutExisting, activeGroupIndex: destinationIndex }
        }),
    closeMarkdownPreviewTab: (groupIndex, previewPath) =>
        set((s) => {
            const groups = s.groups.map((group) => ({ ...group, tabs: [...group.tabs] }))
            const group = groups[groupIndex]
            if (!group) return s
            const tab = group.tabs.find((candidate) => candidate.path === previewPath)
            if (!tab || !isMarkdownPreviewTab(tab)) return s
            group.tabs = group.tabs.filter((candidate) => candidate.path !== previewPath)
            if (group.activePath === previewPath) {
                group.activePath = group.tabs.at(-1)?.path ?? null
            }
            return pruneEmptiedGroups(
                groups,
                group.tabs.length === 0 ? [groupIndex] : [],
                s.activeGroupIndex
            )
        }),
    closeMarkdownPreviewsForSource: (sourcePath) =>
        set((s) => closeMarkdownPreviewsInGroups(s.groups, sourcePath, s.activeGroupIndex)),
    hasMarkdownPreview: (sourcePath) => findMarkdownPreview(get().groups, sourcePath) !== null,
    openHerdrTerminalPage: ({
        herdrSessionId,
        runtimeTarget,
        terminalId,
        title,
        paneId,
        herdrTabId,
        herdrWorkspaceId,
        groupIndex
    }) =>
        set((s) => {
            const resolvedRuntimeTarget = normalizeHerdrRuntimeTarget(runtimeTarget)
            const path = herdrPagePath(herdrSessionId, terminalId, resolvedRuntimeTarget)
            const tabId = herdrTabId?.trim() || null
            let existingGroupIndex = -1
            let existingPath: string | null = null
            for (let gi = 0; gi < s.groups.length; gi++) {
                const group = s.groups[gi]
                for (const tab of group.tabs) {
                    if (tab.kind !== "herdr-terminal") continue
                    if (tab.herdrSessionId !== herdrSessionId) continue
                    if (!sameHerdrRuntimeTarget(tab.herdrRuntimeTarget, resolvedRuntimeTarget)) continue
                    if (tabId && tab.herdrTabId && tab.herdrTabId === tabId) {
                        existingGroupIndex = gi
                        existingPath = tab.path
                        break
                    }
                    if (!tabId && tab.path === path) {
                        existingGroupIndex = gi
                        existingPath = tab.path
                        break
                    }
                    // Legacy pages without tabId still match by terminalId.
                    if (tabId && !tab.herdrTabId && tab.terminalId === terminalId) {
                        existingGroupIndex = gi
                        existingPath = tab.path
                        break
                    }
                }
                if (existingGroupIndex !== -1) break
            }
            if (existingGroupIndex !== -1 && existingPath) {
                const focusPath = existingPath
                return {
                    groups: s.groups.map((group, index) => {
                        if (index !== existingGroupIndex) return group
                        return {
                            ...group,
                            activePath: focusPath,
                            tabs: group.tabs.map((tab) =>
                                tab.path === focusPath
                                    ? {
                                          ...tab,
                                          name: title ?? tab.name,
                                          paneId:
                                              paneId !== undefined ? paneId : tab.paneId,
                                          herdrTabId: tabId ?? tab.herdrTabId ?? null,
                                          herdrWorkspaceId:
                                              herdrWorkspaceId !== undefined
                                                  ? herdrWorkspaceId
                                                  : tab.herdrWorkspaceId ?? null,
                                          kind: "herdr-terminal" as const,
                                          herdrSessionId,
                                          herdrRuntimeTarget: resolvedRuntimeTarget,
                                          // Keep original path identity; refresh live metadata.
                                          terminalId: tab.terminalId ?? terminalId
                                      }
                                    : tab
                            )
                        }
                    }),
                    activeGroupIndex: existingGroupIndex
                }
            }
            const targetGroupIndex = groupIndex ?? s.activeGroupIndex
            if (!s.groups[targetGroupIndex]) return s
            const groups = s.groups.map((g) => ({ ...g, tabs: [...g.tabs] }))
            const g = groups[targetGroupIndex]
            g.tabs.push({
                path,
                name: title?.trim() || terminalId,
                dirty: false,
                externallyModified: false,
                kind: "herdr-terminal",
                herdrSessionId,
                herdrRuntimeTarget: resolvedRuntimeTarget,
                terminalId,
                herdrTabId: tabId,
                herdrWorkspaceId: herdrWorkspaceId ?? null,
                paneId: paneId ?? null
            })
            g.activePath = path
            return { groups, activeGroupIndex: targetGroupIndex }
        }),
    updateHerdrPagePaneId: (pagePath, paneId) =>
        set((s) => {
            if (!isHerdrPagePath(pagePath)) return s
            let changed = false
            const groups = s.groups.map((group) => ({
                ...group,
                tabs: group.tabs.map((tab) => {
                    if (tab.path !== pagePath) return tab
                    if (tab.paneId === paneId) return tab
                    changed = true
                    return { ...tab, paneId, kind: "herdr-terminal" as const }
                })
            }))
            return changed ? { groups } : s
        }),
    updateHerdrPageTabId: (pagePath, herdrTabId) =>
        set((s) => {
            if (!isHerdrPagePath(pagePath) || !herdrTabId.trim()) return s
            let changed = false
            const groups = s.groups.map((group) => ({
                ...group,
                tabs: group.tabs.map((tab) => {
                    if (tab.path !== pagePath || tab.kind !== "herdr-terminal") return tab
                    if (tab.herdrTabId === herdrTabId) return tab
                    changed = true
                    return { ...tab, herdrTabId }
                })
            }))
            return changed ? { groups } : s
        }),
    updateHerdrPageTitle: (pagePath, title) =>
        set((s) => {
            if (!isHerdrPagePath(pagePath) || !title.trim()) return s
            let changed = false
            const groups = s.groups.map((group) => ({
                ...group,
                tabs: group.tabs.map((tab) => {
                    if (tab.path !== pagePath || tab.kind !== "herdr-terminal") return tab
                    if (tab.name === title) return tab
                    changed = true
                    return { ...tab, name: title }
                })
            }))
            return changed ? { groups } : s
        }),
    setActiveTab: (groupIndex, path) =>
        set((s) => {
            const groups = s.groups.map((g, i) =>
                i === groupIndex ? { ...g, activePath: path } : g
            )
            return { groups, activeGroupIndex: groupIndex }
        }),
    setActiveGroup: (groupIndex) =>
        set((s) => (s.groups[groupIndex] ? { activeGroupIndex: groupIndex } : s)),
    markDirty: (path, dirty) =>
        set((s) => ({
            groups: s.groups.map((g) => ({
                ...g,
                tabs: g.tabs.map((t) => (t.path === path ? { ...t, dirty } : t))
            }))
        })),
    hydrateLineEnding: (path, lineEnding, generation) =>
        set((s) => ({
            groups: s.groups.map((g) => ({
                ...g,
                tabs: g.tabs.map((t) =>
                    t.path === path &&
                    (t.lineEndingGeneration === undefined || generation > t.lineEndingGeneration)
                        ? { ...t, lineEnding, lineEndingGeneration: generation }
                        : t
                )
            }))
        })),
    setLineEnding: (path, lineEnding) =>
        set((s) => ({
            groups: s.groups.map((g) => ({
                ...g,
                tabs: g.tabs.map((t) =>
                    t.path === path && t.lineEnding !== lineEnding
                        ? { ...t, lineEnding, dirty: true }
                        : t
                )
            }))
        })),
    getLineEnding: (path) => {
        for (const group of get().groups) {
            const tab = group.tabs.find((candidate) => candidate.path === path)
            if (tab?.lineEnding) return tab.lineEnding
        }
        return undefined
    },
    markExternallyModified: (path, modified) =>
        set((s) => ({
            groups: s.groups.map((g) => ({
                ...g,
                tabs: g.tabs.map((t) =>
                    t.path === path ? { ...t, externallyModified: modified } : t
                )
            }))
        })),
    splitRight: () =>
        set((s) => (s.groups.length >= 2 ? s : { groups: [...s.groups, emptyGroup()] })),
    closeSplit: () =>
        set((s) => {
            if (s.groups.length < 2) return s
            const removedSources = s.groups[1].tabs.filter(isFileTab).map((tab) => tab.path)
            let groups = [s.groups[0]]
            let activeGroupIndex = 0
            for (const source of removedSources) {
                const next = closeMarkdownPreviewsInGroups(groups, source, activeGroupIndex)
                groups = next.groups
                activeGroupIndex = next.activeGroupIndex
            }
            return { groups, activeGroupIndex: 0 }
        }),
    requestReveal: (path, line, focus, groupIndex) => {
        if (groupIndex === undefined) get().openTab(path)
        else get().openTabInGroup(path, groupIndex)
        // Store focus only when specified so callers that omit it keep the default
        // (consumed as `?? true`) and the pendingReveal shape stays minimal.
        set({ pendingReveal: focus === undefined ? { path, line } : { path, line, focus } })
    },
    consumeReveal: () => set({ pendingReveal: null })
}))

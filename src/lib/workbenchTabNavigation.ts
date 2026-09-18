import { useHerdrStore } from "@/state/herdrStore"
import { useUiStore } from "@/state/uiStore"
import { useWorkspaceStore, type TabInfo } from "@/state/workspaceStore"
import type { HerdrTabInfo } from "./herdrTypes"

export type TabNavigation = { index: number } | { direction: 1 | -1 }
let navigationIntent = 0

export function visibleWorkbenchTabs(groupIndex: number): TabInfo[] {
    const group = useWorkspaceStore.getState().groups[groupIndex]
    const runtime = useHerdrStore.getState()
    if (!group) return []
    return group.tabs.filter((tab) => {
        if (tab.kind !== "herdr-terminal" || !runtime.selectedSessionName || !runtime.selectedSpaceId) return true
        const session = tab.herdrSessionId === "live"
            ? (runtime.sessions.find((session) => session.default) ?? runtime.sessions[0])?.name ?? "live"
            : tab.herdrSessionId ?? "live"
        const workspace = tab.herdrWorkspaceId ?? runtime.snapshot?.tabs.find((item) => item.id === tab.herdrTabId)?.workspaceId
        return session === runtime.selectedSessionName && workspace === runtime.selectedSpaceId
    }).sort((a, b) => Number(Boolean(b.pinned)) - Number(Boolean(a.pinned)))
}

export function tabNavigationIndex(activeIndex: number, count: number, request: TabNavigation): number | null {
    if (!count) return null
    if ("index" in request) return request.index >= 0 && request.index < count ? request.index : null
    if (activeIndex < 0) return request.direction === 1 ? 0 : count - 1
    return (activeIndex + request.direction + count) % count
}

export async function navigateWorkbenchTabs(request: TabNavigation): Promise<void> {
    if (!["files", "ade"].includes(useUiStore.getState().mode)) return
    const state = useWorkspaceStore.getState()
    const groupIndex = state.activeGroupIndex
    const group = state.groups[groupIndex]
    const tabs = visibleWorkbenchTabs(groupIndex)
    const index = tabNavigationIndex(tabs.findIndex((tab) => tab.path === group?.activePath), tabs.length, request)
    if (index === null) return
    await activateWorkbenchTab(groupIndex, tabs[index])
}

export function activateRuntimeWorkbenchTab(tab: HerdrTabInfo) {
    navigationIntent++
    return useHerdrStore.getState().activateTab(tab)
}

export async function activateWorkbenchTab(groupIndex: number, tab: TabInfo, runtimeTab?: HerdrTabInfo): Promise<void> {
    const state = useWorkspaceStore.getState()
    const group = state.groups[groupIndex]
    const intent = ++navigationIntent
    const previousPath = group?.activePath
    state.setActiveTab(groupIndex, tab.path)
    if (tab.kind !== "herdr-terminal") return
    const runtime = useHerdrStore.getState()
    const target = runtimeTab ?? runtime.snapshot?.tabs.find((item) => item.id === tab.herdrTabId)
    if (!target) return
    const result = await runtime.activateTab(target)
    const live = useWorkspaceStore.getState()
    if (result?.ok === false && navigationIntent === intent && previousPath && live.workspacePath === state.workspacePath && live.activeGroupIndex === groupIndex && live.groups[groupIndex]?.activePath === tab.path) {
        live.setActiveTab(groupIndex, previousPath)
    }
}

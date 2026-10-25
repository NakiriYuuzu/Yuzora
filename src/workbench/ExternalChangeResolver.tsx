import { lazy, Suspense } from "react"
import { getView, subscribeView } from "../editor/viewRegistry"
import { useUiStore } from "../state/uiStore"
import { useWorkspaceStore } from "../state/workspaceStore"

const ResolverBody = lazy(() => import("./ExternalChangeResolverBody").then(m => ({ default: m.ResolverBody })))

// Pure interception predicate exported for EditorPane's save closure. When the
// target tab is flagged externallyModified, open the resolver and swallow the
// save; otherwise let the normal save proceed.
export function maybeInterceptSave(path: string): boolean {
    const flagged = useWorkspaceStore
        .getState()
        .groups.some((g) => g.tabs.some((t) => t.path === path && t.externallyModified))
    if (!flagged) return false
    useUiStore.getState().openResolver(path)
    return true
}

// Tab-strip ↻ entry. Only a group's active tab has a mounted editor, and the
// resolver closes at once without a live view, so a background tab is brought
// forward first and the resolver opens when its editor registers. The wait is
// abandoned if the user moves to another tab before that.
export function openResolverForTab(groupIndex: number, path: string): void {
    const group = useWorkspaceStore.getState().groups[groupIndex]
    if (!group?.tabs.some((tab) => tab.path === path)) return
    if (group.activePath === path || getView(path)) {
        useUiStore.getState().openResolver(path)
        return
    }
    const stopView = subscribeView(path, () => {
        if (!getView(path)) return
        stop()
        useUiStore.getState().openResolver(path)
    })
    const stopTabs = useWorkspaceStore.subscribe((state) => {
        if (state.groups[groupIndex]?.activePath !== path) stop()
    })
    function stop() {
        stopView()
        stopTabs()
    }
    useWorkspaceStore.getState().setActiveTab(groupIndex, path)
}

export function ExternalChangeResolver() {
    const resolverPath = useUiStore((s) => s.resolverPath)
    const session = useUiStore((s) => s.resolverNonce)
    if (!resolverPath) return null
    return <Suspense fallback={null}><ResolverBody key={`${session}:${resolverPath}`} path={resolverPath} session={session} /></Suspense>
}

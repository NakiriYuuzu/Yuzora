import { lazy, Suspense } from "react"
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

export function ExternalChangeResolver() {
    const resolverPath = useUiStore((s) => s.resolverPath)
    if (!resolverPath) return null
    return <Suspense fallback={null}><ResolverBody key={resolverPath} path={resolverPath} /></Suspense>
}

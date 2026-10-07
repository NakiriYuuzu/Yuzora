import type { TabInfo } from "../state/workspaceStore"
import { relativePathWithin } from "./paths"

export interface ExternalChangePlan {
    reload: string[]
    markModified: string[]
}

export function handleExternalChange(
    changedPaths: string[],
    openTabs: TabInfo[],
    recentlySaved: ReadonlySet<string>
): ExternalChangePlan {
    const plan: ExternalChangePlan = { reload: [], markModified: [] }
    const handled = new Set<string>()
    for (const t of openTabs) {
        if (handled.has(t.path) || recentlySaved.has(t.path)) continue
        // Coalesced directory notifications must also reach open descendants.
        if (!changedPaths.some((changed) => relativePathWithin(changed, t.path) !== null)) continue
        handled.add(t.path)
        if (t.dirty) plan.markModified.push(t.path)
        else plan.reload.push(t.path)
    }
    return plan
}

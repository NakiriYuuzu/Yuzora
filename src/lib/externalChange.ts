import type { TabInfo } from "../state/workspaceStore"

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
    for (const path of changedPaths) {
        if (recentlySaved.has(path)) continue
        const t = openTabs.find((tab) => tab.path === path)
        if (!t) continue
        if (t.dirty) plan.markModified.push(path)
        else plan.reload.push(path)
    }
    return plan
}

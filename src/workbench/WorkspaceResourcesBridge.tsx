import { useEffect } from "react"
import { dropDocument } from "@/editor/documentRegistry"
import { isFileTab } from "@/lib/markdownPreviewTab"
import { useSvgPreviewStore } from "@/state/svgPreviewStore"
import { useWorkspaceStore, type EditorGroup } from "@/state/workspaceStore"
import { forgetRemoteFileRevision, releaseRemoteWorkspace } from "@/lib/remoteFiles"

// Metadata and focus changes leave document ownership unchanged. Keeping this
// comparison separate also avoids per-group callbacks on structural updates.
function sameFileOwners(groups: EditorGroup[], previous: EditorGroup[]): boolean {
    if (groups === previous) return true
    if (groups.length !== previous.length) return false
    for (let groupIndex = 0; groupIndex < groups.length; groupIndex++) {
        const group = groups[groupIndex]
        const before = previous[groupIndex]
        if (group === before) continue
        if (!group || !before) return false
        const tabs = group.tabs
        const oldTabs = before.tabs
        if (tabs === oldTabs) continue
        if (tabs.length !== oldTabs.length) return false
        for (let tabIndex = 0; tabIndex < tabs.length; tabIndex++) {
            const tab = tabs[tabIndex]
            const oldTab = oldTabs[tabIndex]
            if (tab === oldTab) continue
            if (!tab || !oldTab || tab.path !== oldTab.path || isFileTab(tab) !== isFileTab(oldTab)) return false
        }
    }
    return true
}

/**
 * Retire documents and their SVG preview state when their last tab disappears,
 * including a replaced preview-mode tab or an entire split.
 */
export function WorkspaceResourcesBridge() {
    useEffect(() => useWorkspaceStore.subscribe((state, previous) => {
        if (state.workspacePath === previous.workspacePath && sameFileOwners(state.groups, previous.groups)) return
        const open = new Set(state.groups.flatMap(group => group.tabs.filter(isFileTab).map(tab => tab.path)))
        for (const group of previous.groups) for (const tab of group.tabs) {
            if (isFileTab(tab) && (state.workspacePath !== previous.workspacePath || !open.has(tab.path))) {
                dropDocument(tab.path, previous.workspacePath)
                forgetRemoteFileRevision(tab.path)
                useSvgPreviewStore.getState().forget(tab.path)
            }
        }
        if (previous.workspacePath && previous.workspacePath !== state.workspacePath) {
            void releaseRemoteWorkspace(previous.workspacePath)
        }
    }), [])
    return null
}

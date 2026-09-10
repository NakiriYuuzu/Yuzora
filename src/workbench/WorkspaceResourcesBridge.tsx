import { useEffect } from "react"
import { dropDocument } from "@/editor/documentRegistry"
import { isFileTab } from "@/lib/markdownPreviewTab"
import { useWorkspaceStore } from "@/state/workspaceStore"
import { forgetRemoteFileRevision, releaseRemoteWorkspace } from "@/lib/remoteFiles"

/** Retire documents when their last tab disappears, including an entire split. */
export function WorkspaceResourcesBridge() {
    useEffect(() => useWorkspaceStore.subscribe((state, previous) => {
        if (state.groups === previous.groups && state.workspacePath === previous.workspacePath) return
        const open = new Set(state.groups.flatMap(group => group.tabs.filter(isFileTab).map(tab => tab.path)))
        for (const group of previous.groups) for (const tab of group.tabs) {
            if (isFileTab(tab) && (state.workspacePath !== previous.workspacePath || !open.has(tab.path))) {
                dropDocument(tab.path, previous.workspacePath)
                forgetRemoteFileRevision(tab.path)
            }
        }
        if (previous.workspacePath && previous.workspacePath !== state.workspacePath) {
            void releaseRemoteWorkspace(previous.workspacePath)
        }
    }), [])
    return null
}

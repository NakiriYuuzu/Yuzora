import { create } from "zustand"

import { useWorkspaceStore } from "./workspaceStore"

// SVG preview visibility — inverted from Markdown's store on purpose: an SVG
// opens with its preview visible by default (user decision, plan t3-3b), so
// state records the *explicitly closed* paths. Session-scoped like Markdown's:
// reset on workspace switch, forgotten when the tab closes. Lives under state/
// (not in SvgSplitView.tsx) so store-only consumers like contextMenuStore
// don't pull the editor component graph in with it.
interface SvgPreviewState {
    closedPaths: Record<string, boolean>
    toggle: (path: string) => void
    forget: (path: string) => void
    reset: () => void
    isOpen: (path: string) => boolean
}

export const useSvgPreviewStore = create<SvgPreviewState>((set, get) => ({
    closedPaths: {},
    toggle: (path) =>
        set((s) => ({ closedPaths: { ...s.closedPaths, [path]: !s.closedPaths[path] } })),
    forget: (path) =>
        set((s) => {
            if (!(path in s.closedPaths)) return s
            const next = { ...s.closedPaths }
            delete next[path]
            return { closedPaths: next }
        }),
    reset: () => set({ closedPaths: {} }),
    isOpen: (path) => !get().closedPaths[path]
}))

// Workspace 切換清空開關狀態，避免無界累積與跨專案殘留（比照 MarkdownPreview W8）。
useWorkspaceStore.subscribe((s, prev) => {
    if (s.workspacePath !== prev.workspacePath) useSvgPreviewStore.getState().reset()
})

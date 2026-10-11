import { create } from "zustand"

/** Files copied or cut from the file tree, waiting to be pasted. */
export interface FileClipboard {
    workspacePath: string
    paths: string[]
    mode: "copy" | "cut"
    /** WSL copies never reach the OS clipboard; its file list at copy time tells a later Explorer copy apart. */
    osSnapshot?: string[]
}

interface FileClipboardState {
    clipboard: FileClipboard | null
    /** Last file-tree row the user clicked or opened a menu on. */
    selection: { workspacePath: string; path: string } | null
    setClipboard: (clipboard: FileClipboard | null) => void
    select: (workspacePath: string, path: string) => void
}

export const useFileClipboardStore = create<FileClipboardState>((set) => ({
    clipboard: null,
    selection: null,
    setClipboard: (clipboard) => set({ clipboard }),
    select: (workspacePath, path) => set({ selection: { workspacePath, path } })
}))

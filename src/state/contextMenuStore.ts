import type { MouseEvent as ReactMouseEvent } from "react"
import { create } from "zustand"

import { logUserAction } from "../lib/ipc"
import { worktreeFilesFrom } from "../workbench/git/fileRows"
import { useDiffModalStore, type WorktreeDiffFile } from "./diffModalStore"
import { useGitStore } from "./gitStore"
import { useUiStore } from "./uiStore"
import { useWorkspaceStore } from "./workspaceStore"

export type ContextMenuKind =
    | "general"
    | "rail"
    | "explorer"
    | "file"
    | "tab"
    | "editor"
    | "terminal"
    | "agent"
    | "git"
    | "status"
    | "sshhost"

export interface ContextMenuPayload {
    path?: string
    groupIndex?: number
    host?: string
}

interface ContextMenuState {
    kind: ContextMenuKind | null
    x: number
    y: number
    payload: ContextMenuPayload
    open: (kind: ContextMenuKind, x: number, y: number, payload?: ContextMenuPayload) => void
    close: () => void
}

// x/y 存 pointer 的 visual px；換算 layout px（body zoom）由 ContextMenu 元件
// 在 render 時處理，store 不需要知道縮放。
export const useContextMenuStore = create<ContextMenuState>((set) => ({
    kind: null,
    x: 0,
    y: 0,
    payload: {},
    open: (kind, x, y, payload = {}) => set({ kind, x, y, payload }),
    close: () => set({ kind: null })
}))

// 每個區域共用的 handler factory：右鍵開啟該區選單、擋掉 WebView 原生選單、
// 停止冒泡（否則外層區域的 handler 會蓋掉這次 open）。
export function contextMenuHandler(kind: ContextMenuKind, payload?: ContextMenuPayload) {
    return (event: ReactMouseEvent) => {
        event.preventDefault()
        event.stopPropagation()
        useContextMenuStore.getState().open(kind, event.clientX, event.clientY, payload)
    }
}

// 設計要求 preview 面板整個吃掉右鍵：不彈選單、也不冒泡到 general。
export function suppressContextMenu(event: ReactMouseEvent) {
    event.preventDefault()
    event.stopPropagation()
}

// Repo-relative form of an absolute editor-tab path (mirrors FileTree's
// relativePath). git status paths are repo-relative; editor tabs open with
// absolute paths (FileTree passes node.path), so we must strip the repo root
// before comparing against worktreeFilesFrom. Returns null when the repo is not
// ready or the path is outside the repo root (→ Compare is a no-op).
function relativeToRepoRoot(activePath: string): string | null {
    const environment = useGitStore.getState().environment
    if (environment?.status !== "ready") return null
    const root = environment.root
    if (!activePath.startsWith(root + "/")) return null
    return activePath.slice(root.length + 1)
}

// The active editor file (as a repo-relative path) + flattened worktree file
// list (staged first, then the working-tree changes — matching the design's
// openDiffWork ordering). Returns null when no file is active, the repo is not
// ready, the active file is outside the repo, or it has no git change.
function worktreeCompareTarget(): { files: WorktreeDiffFile[]; activePath: string } | null {
    const ws = useWorkspaceStore.getState()
    const activePath = ws.groups[ws.activeGroupIndex]?.activePath ?? null
    if (!activePath) return null

    const status = useGitStore.getState().status
    if (!status) return null

    const rel = relativeToRepoRoot(activePath)
    if (!rel) return null

    const files: WorktreeDiffFile[] = worktreeFilesFrom(status)

    // Only enabled for files that are in the git change set (design semantics:
    // Compare with HEAD is meaningless for an unchanged file).
    if (!files.some((f) => f.path === rel)) return null
    return { files, activePath: rel }
}

// The single dispatch seam: every menu item routes here. Most items are still
// UI-only (close + log); real actions are wired in per item as capabilities land
// — cmCompareHead opens the Diff modal on the active file's working-tree diff.
export function runContextMenuAction(
    kind: ContextMenuKind,
    actionId: string,
    payload: ContextMenuPayload
) {
    useContextMenuStore.getState().close()
    void logUserAction("context_menu_action", `${kind}:${actionId}`, { ...payload })

    if (actionId === "cmCompareHead") {
        const target = worktreeCompareTarget()
        if (!target) return
        useUiStore.getState().setMode("git")
        // Path-only active target (string overload) keeps the existing
        // Compare-with-HEAD semantics: open the first row for this path. For an MM
        // file that is the staged side (worktreeFilesFrom lists staged first) —
        // acceptable here, the menu compares the file against HEAD regardless of
        // side, and the user can switch sides in the modal's file list.
        useDiffModalStore.getState().openWorktree(target.files, target.activePath)
    }
}

import { confirm, message } from "@tauri-apps/plugin-dialog"
import { revealItemInDir } from "@tauri-apps/plugin-opener"
import { readText, writeText } from "@tauri-apps/plugin-clipboard-manager"
import { formatDocument } from "@codemirror/lsp-client"
import type { EditorView } from "@codemirror/view"
import type { MouseEvent as ReactMouseEvent } from "react"
import { create } from "zustand"

import { dropDocument, renameDocument } from "../editor/documentRegistry"
import { getView } from "../editor/viewRegistry"
import { logUserAction } from "@/features/logs/userAction"
import {
    fsCreateDir,
    fsCreateFile,
    fsDelete,
    fsRename,
    gitFetch,
    gitPull,
    gitPush,
    previewServe
} from "../lib/ipc"
import { useMarkdownPreviewStore } from "../workbench/MarkdownPreview"
import { usePreviewStore } from "./previewStore"
import { worktreeFilesFrom } from "../workbench/git/fileRows"
import { useAgentStore } from "./agentStore"
import { useDiffModalStore, type WorktreeDiffFile } from "./diffModalStore"
import { useGitStore } from "./gitStore"
import { useSftpStore } from "./sftpStore"
import { useSshStore } from "./sshStore"
import { useTerminalStore } from "./terminalStore"
import { useUiStore } from "./uiStore"
import { useWorkspaceStore, type TabInfo } from "./workspaceStore"

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
    // sshhost rows carry the host id so cmOpenSsh/cmDisconnect can reach sshStore.
    hostId?: string
    // Set by FileTree's "file" entries so cmDelete can word its confirm text for
    // a folder ("將刪除整個資料夾") vs a single file.
    isDir?: boolean
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

// Repo-relative form used by cmCompareHead compares against the git root;
// tab/file "Copy relative path" instead reads relative to the *workspace*
// (the folder the user opened), per spec — the two roots can differ (a repo
// opened at a subdirectory). Falls back to the absolute path when it isn't
// under the workspace (e.g. no workspace open yet).
function relativeToWorkspace(absPath: string): string {
    const workspacePath = useWorkspaceStore.getState().workspacePath
    if (workspacePath && absPath.startsWith(workspacePath + "/")) {
        return absPath.slice(workspacePath.length + 1)
    }
    return absPath
}

// The active editor's CodeMirror view, resolved via the shared view registry
// (EditorPane registers/unregisters on mount/unmount) rather than through a
// specific pane's local ref — the dispatch here lives outside any one pane's
// component tree.
function activeEditorView(): EditorView | null {
    const ws = useWorkspaceStore.getState()
    const path = ws.groups[ws.activeGroupIndex]?.activePath
    if (!path) return null
    return getView(path) ?? null
}

function selectedText(view: EditorView): string {
    return view.state.selection.ranges
        .filter((range) => !range.empty)
        .map((range) => view.state.sliceDoc(range.from, range.to))
        .join("\n")
}

async function copySelection(view: EditorView): Promise<void> {
    const text = selectedText(view)
    if (!text) return
    await writeText(text)
}

async function cutSelection(view: EditorView): Promise<void> {
    const text = selectedText(view)
    if (!text) return
    await writeText(text)
    view.dispatch(view.state.replaceSelection(""))
}

async function pasteIntoEditor(view: EditorView): Promise<void> {
    const text = await readText()
    if (!text) return
    view.dispatch(view.state.replaceSelection(text))
}

// Cleanup that mirrors TabBar's onClose for a single non-preview tab (minus
// the store-level closeTab mutation itself, which the caller does in bulk).
function dropTabSideEffects(tab: TabInfo): void {
    dropDocument(tab.path)
    useMarkdownPreviewStore.getState().close(tab.path)
}

// Single-tab close, replicating TabBar's onClose confirm flow so the tab
// context menu's "Close tab" behaves identically to clicking the tab's own
// close button.
async function closeTabWithConfirm(groupIndex: number, path: string): Promise<void> {
    const group = useWorkspaceStore.getState().groups[groupIndex]
    const tab = group?.tabs.find((t) => t.path === path)
    if (!tab) return
    if (tab.kind === "preview") {
        useWorkspaceStore.getState().closePreviewTab()
        return
    }
    if (tab.dirty) {
        const ok = await confirm("檔案有未儲存的變更，確定關閉？")
        if (!ok) return
    }
    useWorkspaceStore.getState().closeTab(groupIndex, path)
    dropTabSideEffects(tab)
}

// "Close others" / "Close all": one combined confirm when any target tab is
// dirty (rather than TabBar's per-tab prompt) — closing a batch of tabs one
// dialog at a time would be tedious; declining leaves every tab open.
async function closeOtherTabsWithConfirm(groupIndex: number, keepPath: string): Promise<void> {
    const group = useWorkspaceStore.getState().groups[groupIndex]
    if (!group) return
    const toClose = group.tabs.filter((t) => t.path !== keepPath)
    if (toClose.length === 0) return
    if (toClose.some((t) => t.kind !== "preview" && t.dirty)) {
        const ok = await confirm("有分頁未儲存的變更，確定全部關閉？")
        if (!ok) return
    }
    useWorkspaceStore.getState().closeOtherTabs(groupIndex, keepPath)
    for (const t of toClose) {
        if (t.kind !== "preview") dropTabSideEffects(t)
    }
}

async function closeAllTabsWithConfirm(groupIndex: number): Promise<void> {
    const group = useWorkspaceStore.getState().groups[groupIndex]
    if (!group || group.tabs.length === 0) return
    if (group.tabs.some((t) => t.kind !== "preview" && t.dirty)) {
        const ok = await confirm("有分頁未儲存的變更，確定全部關閉？")
        if (!ok) return
    }
    const tabs = group.tabs
    useWorkspaceStore.getState().closeAllTabs(groupIndex)
    for (const t of tabs) {
        if (t.kind !== "preview") dropTabSideEffects(t)
    }
}

// --- explorer/file: filesystem operations (PROB-5 後波) ---
// Name input reuses the imperative window.prompt (mirroring this module's
// imperative confirm() usage) — the repo has no in-app text-input dialog
// primitive. Every op validates the workspace boundary in Rust; failures are
// surfaced through a dialog message rather than swallowed. On success we bump
// treeRevision (FileTree doesn't subscribe to the fs watcher for its own ops).
function joinName(dir: string, name: string): string {
    return `${dir.replace(/\/+$/, "")}/${name}`
}

async function createEntry(kind: "file" | "folder"): Promise<void> {
    const ws = useWorkspaceStore.getState()
    const workspace = ws.workspacePath
    if (!workspace) return
    const name = window.prompt(kind === "file" ? "輸入新檔案名稱" : "輸入新資料夾名稱")?.trim()
    if (!name) return
    const target = joinName(workspace, name)
    try {
        if (kind === "file") {
            await fsCreateFile(workspace, target)
            useWorkspaceStore.getState().refreshTree()
            useWorkspaceStore.getState().openTab(target)
        } else {
            await fsCreateDir(workspace, target)
            useWorkspaceStore.getState().refreshTree()
        }
    } catch (e) {
        await message(String(e), { title: "建立失敗", kind: "error" })
    }
}

// Paths of every open (non-preview) tab, across ALL groups, that an fs op on
// `target` touches: the file itself, or — when target is a folder — everything
// beneath it. Deduped, since the same file can be open in multiple split groups.
function affectedTabPaths(target: string): string[] {
    const paths = new Set<string>()
    for (const g of useWorkspaceStore.getState().groups) {
        for (const t of g.tabs) {
            if (t.kind === "preview") continue
            if (t.path === target || t.path.startsWith(target + "/")) paths.add(t.path)
        }
    }
    return [...paths]
}

async function renameEntry(path: string): Promise<void> {
    const workspace = useWorkspaceStore.getState().workspacePath
    if (!workspace) return
    const currentName = path.split("/").pop() ?? path
    const name = window.prompt("輸入新名稱", currentName)?.trim()
    if (!name || name === currentName) return
    const slash = path.lastIndexOf("/")
    const target = path.slice(0, slash + 1) + name
    try {
        await fsRename(workspace, path, target)
        // Re-point every open tab + its editor state from the old path to the new
        // one, so a later save lands on the renamed file instead of recreating the
        // old path (Finding 3). Move the registry entry (snapshotting the live
        // buffer so unsaved edits survive) and the markdown-preview toggle BEFORE
        // updateTabPath triggers the EditorPane remount at the new key.
        const preview = useMarkdownPreviewStore.getState()
        for (const oldPath of affectedTabPaths(path)) {
            const newPath = oldPath === path ? target : target + oldPath.slice(path.length)
            renameDocument(oldPath, newPath, getView(oldPath)?.state.doc.toString())
            if (preview.isOpen(oldPath)) {
                preview.close(oldPath)
                if (!preview.isOpen(newPath)) preview.toggle(newPath)
            }
        }
        useWorkspaceStore.getState().updateTabPath(path, target)
        useWorkspaceStore.getState().refreshTree()
    } catch (e) {
        await message(String(e), { title: "重新命名失敗", kind: "error" })
    }
}

async function deleteEntry(path: string, isDir: boolean): Promise<void> {
    const workspace = useWorkspaceStore.getState().workspacePath
    if (!workspace) return
    const name = path.split("/").pop() ?? path
    // The delete confirm below already covers the destructive intent (the file's
    // content — dirty buffer included — is going away regardless), so no separate
    // per-tab dirty prompt: it would be redundant once the user confirms delete.
    const text = isDir
        ? `確定刪除「${name}」？將刪除整個資料夾及其內容。`
        : `確定刪除「${name}」？`
    const ok = await confirm(text, { title: "刪除", kind: "warning" })
    if (!ok) return
    try {
        await fsDelete(workspace, path)
        // Close every tab that pointed at the deleted file/folder and drop its
        // editor + preview state, so a stale tab can't recreate the file on its
        // next save (Finding 3).
        const affected = affectedTabPaths(path)
        if (affected.length > 0) {
            useWorkspaceStore.getState().closeTabsByPath(affected)
            for (const p of affected) {
                dropDocument(p)
                useMarkdownPreviewStore.getState().close(p)
            }
        }
        useWorkspaceStore.getState().refreshTree()
    } catch (e) {
        await message(String(e), { title: "刪除失敗", kind: "error" })
    }
}

// cmOpenInBrowser (P3): serve the HTML file's own directory over a local http
// origin (so relative css/js/module specifiers resolve) and open that URL in the
// singleton preview tab. Same dir reuses one server/port on the Rust side.
async function openInBrowser(path: string): Promise<void> {
    const workspace = useWorkspaceStore.getState().workspacePath
    if (!workspace) return
    // Split on either separator so Windows paths (C:\dir\file.html) serve the
    // right directory and produce a valid URL basename.
    const sep = Math.max(path.lastIndexOf("/"), path.lastIndexOf("\\"))
    const dir = sep > 0 ? path.slice(0, sep) : workspace
    const fileName = sep >= 0 ? path.slice(sep + 1) : path
    try {
        const port = await previewServe(dir)
        const url = `http://127.0.0.1:${port}/${encodeURIComponent(fileName)}`
        useWorkspaceStore.getState().openPreviewTab()
        usePreviewStore.getState().navigate(workspace, url)
    } catch (e) {
        await message(String(e), { title: "無法開啟預覽", kind: "error" })
    }
}

// The single dispatch seam: every menu item routes here. Real actions are
// wired in per item as capabilities land — the rest stay UI-only (close +
// log) until a backend capability (or larger UX, e.g. destructive-op confirm)
// lands; see PROB-5 deferred list.
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
        return
    }

    // --- general / rail (shared switches: same behaviour wherever the item
    // appears, so no kind check) ---
    if (actionId === "cmSettings") {
        useUiStore.getState().openSettings()
        return
    }
    if (actionId === "cmHideSidebar") {
        useUiStore.getState().requestSidebarToggle()
        return
    }
    if (actionId === "cmCmdPalette") {
        useUiStore.getState().requestOpenPalette()
        return
    }

    // --- tab ---
    if (
        kind === "tab" &&
        actionId === "cmCloseTab" &&
        payload.path !== undefined &&
        payload.groupIndex !== undefined
    ) {
        void closeTabWithConfirm(payload.groupIndex, payload.path)
        return
    }
    if (
        kind === "tab" &&
        actionId === "cmCloseOthers" &&
        payload.path !== undefined &&
        payload.groupIndex !== undefined
    ) {
        void closeOtherTabsWithConfirm(payload.groupIndex, payload.path)
        return
    }
    if (kind === "tab" && actionId === "cmCloseAll" && payload.groupIndex !== undefined) {
        void closeAllTabsWithConfirm(payload.groupIndex)
        return
    }

    // --- tab / file: shared "copy relative path" ---
    if (actionId === "cmCopyRel" && payload.path !== undefined) {
        void writeText(relativeToWorkspace(payload.path))
        return
    }

    // --- tab / file: "split" — wired to the same groups/split primitive the
    // EditorArea "+" button uses (splitRight only adds an empty second group;
    // it does not itself open the source file into it — no such capability
    // exists yet, see PROB-5 report). ---
    if (actionId === "cmSplit" || actionId === "cmOpenSplit") {
        useWorkspaceStore.getState().splitRight()
        return
    }

    // --- file ---
    if (kind === "file" && actionId === "cmOpen" && payload.path !== undefined) {
        useWorkspaceStore.getState().openTab(payload.path)
        return
    }
    if (kind === "file" && actionId === "cmRename" && payload.path !== undefined) {
        void renameEntry(payload.path)
        return
    }
    if (kind === "file" && actionId === "cmDelete" && payload.path !== undefined) {
        void deleteEntry(payload.path, payload.isDir ?? false)
        return
    }
    if (kind === "file" && actionId === "cmReveal" && payload.path !== undefined) {
        void revealItemInDir(payload.path)
        return
    }
    if (kind === "file" && actionId === "cmOpenInBrowser" && payload.path !== undefined) {
        void openInBrowser(payload.path)
        return
    }

    // --- explorer ---
    if (kind === "explorer" && actionId === "cmCopyPath") {
        const workspacePath = useWorkspaceStore.getState().workspacePath
        if (workspacePath) void writeText(workspacePath)
        return
    }
    // New file/folder create at the workspace root (the explorer menu carries no
    // target dir); cmNewFile opens the freshly-created file in a tab.
    if (kind === "explorer" && actionId === "cmNewFile") {
        void createEntry("file")
        return
    }
    if (kind === "explorer" && actionId === "cmNewFolder") {
        void createEntry("folder")
        return
    }

    // --- editor ---
    if (kind === "editor" && (actionId === "cmCut" || actionId === "cmCopy" || actionId === "cmPaste")) {
        const view = activeEditorView()
        if (!view) return
        if (actionId === "cmCopy") void copySelection(view)
        else if (actionId === "cmCut") void cutSelection(view)
        else void pasteIntoEditor(view)
        return
    }
    if (kind === "editor" && actionId === "cmFormatDoc") {
        const view = activeEditorView()
        if (view) formatDocument(view)
        return
    }

    // --- git / status (shared: same command regardless of which panel) ---
    if ((kind === "git" || kind === "status") && actionId === "cmFetch") {
        void useGitStore.getState().runOp("fetch", () => gitFetch(false))
        return
    }
    if ((kind === "git" || kind === "status") && actionId === "cmPull") {
        void useGitStore.getState().runOp("pull", () => gitPull())
        return
    }
    if ((kind === "git" || kind === "status") && actionId === "cmPush") {
        void useGitStore.getState().runOp("push", () => gitPush())
        return
    }
    if ((kind === "git" || kind === "status") && actionId === "cmCopyBranch") {
        const branch = useGitStore.getState().status?.branch
        if (branch) void writeText(branch)
        return
    }
    if ((kind === "git" || kind === "status") && actionId === "cmCopyHash") {
        const headOid = useGitStore.getState().status?.headOid
        if (headOid) void writeText(headOid)
        return
    }

    // --- agent ---
    if (kind === "agent" && actionId === "cmStop") {
        useAgentStore.getState().cancel()
        return
    }

    // --- sshhost (FEAT-2) ---
    if (kind === "sshhost" && actionId === "cmCopyAddr" && payload.host !== undefined) {
        void writeText(payload.host)
        return
    }
    if (kind === "sshhost" && actionId === "cmOpenSsh" && payload.hostId !== undefined) {
        useSshStore.getState().beginConnect(payload.hostId)
        return
    }
    if (kind === "sshhost" && actionId === "cmDisconnect" && payload.hostId !== undefined) {
        void useSshStore.getState().disconnect(payload.hostId)
        return
    }
    if (kind === "sshhost" && actionId === "cmOpenSftp" && payload.hostId !== undefined) {
        // Reveal the SFTP browser tab and (re)connect the host (F5). The panel's
        // own effect loads the remote listing once the session is connected.
        useSftpStore.getState().openSftp(payload.hostId)
        return
    }

    // --- terminal ---
    if (kind === "terminal" && actionId === "cmKill") {
        const workspace = useWorkspaceStore.getState().workspacePath
        if (!workspace) return
        const layout = useTerminalStore.getState().layouts[workspace]
        const pane = layout?.panes.find((p) => p.paneId === layout.activePaneId) ?? layout?.panes[0]
        if (!pane) return
        useTerminalStore.getState().removeSession(workspace, pane.sessionId)
        return
    }

    // Everything else (cmRefresh, cmNewProject, cmStage/cmRollback,
    // cmDuplicate/cmRenameSession/cmCopyPath[agent], cmClear/cmCopySel/
    // cmPaste[terminal]/cmDockTerm/cmSplitTermRight/cmSplitTermDown) has no
    // reachable front-end-only capability yet — stays UI-only (close + log
    // above). Deferred list + reasons: see PROB-5 report.
}

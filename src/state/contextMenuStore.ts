import { confirm, message } from "@tauri-apps/plugin-dialog"
import { revealItemInDir } from "@tauri-apps/plugin-opener"
import { readText, writeText } from "@tauri-apps/plugin-clipboard-manager"
import type { EditorView } from "@codemirror/view"
import type { MouseEvent as ReactMouseEvent } from "react"
import { create } from "zustand"

import {
    CONTEXT_MENU_CANCELLED,
    CONTEXT_MENU_COMPLETED,
    type ContextMenuCommandOutcome,
    type ContextMenuCommandDefinition,
    type ContextMenuRequest,
    type ContextMenuRunOutcome
} from "@/app/workbench/contextMenuModel"
import i18n from "@/lib/i18n"
import { dropDocument, renameDocument } from "../editor/documentRegistry"
import { getView, getViewEntry, type RegisteredEditorView } from "../editor/viewRegistry"
import { logUserAction } from "@/features/logs/userAction"
import { showActionError } from "@/lib/actionFeedback"
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
import { useDiffModalStore, type WorktreeDiffFile } from "./diffModalStore"
import { useGitStore } from "./gitStore"
import { savedConnectionAddress, useDbStore } from "./dbStore"
import { useSftpStore } from "./sftpStore"
import { useSshStore } from "./sshStore"
import { useUiStore } from "./uiStore"
import { useWorkspaceStore, type TabInfo } from "./workspaceStore"

export type { ContextMenuKind, ContextMenuRequest } from "@/app/workbench/contextMenuModel"

interface ContextMenuState {
    request: ContextMenuRequest | null
    x: number
    y: number
    availabilityRevision: number
    open: (request: ContextMenuRequest, x: number, y: number) => void
    close: () => void
    refreshAvailability: () => void
}

// x/y 存 pointer 的 visual px；換算 layout px（body zoom）由 ContextMenu 元件
// 在 render 時處理，store 不需要知道縮放。
export const useContextMenuStore = create<ContextMenuState>((set) => ({
    request: null,
    x: 0,
    y: 0,
    availabilityRevision: 0,
    open: (request, x, y) => set({ request, x, y }),
    close: () => set({ request: null }),
    refreshAvailability: () => set((state) => ({ availabilityRevision: state.availabilityRevision + 1 }))
}))

// 每個區域共用的 handler factory：右鍵開啟該區選單、擋掉 WebView 原生選單、
// 停止冒泡（否則外層區域的 handler 會蓋掉這次 open）。
export function contextMenuHandler(request: ContextMenuRequest) {
    return (event: ReactMouseEvent) => {
        event.preventDefault()
        event.stopPropagation()
        useContextMenuStore.getState().open(request, event.clientX, event.clientY)
    }
}

export async function runContextMenuAction(
    request: ContextMenuRequest,
    command: ContextMenuCommandDefinition
): Promise<ContextMenuRunOutcome> {
    const menu = useContextMenuStore.getState()
    if (menu.request !== null && menu.request !== request) return "cancelled"

    const availability = command.availability(request)
    if (!availability.visible || !availability.enabled) {
        menu.refreshAvailability()
        return "cancelled"
    }

    menu.close()
    const label = command.label(request)
    try {
        const outcome = await command.executor(request)
        if (outcome === "completed") {
            void logUserAction("context_menu_action", `${request.kind}:${command.id}`, { ...request })
        }
        return outcome
    } catch (error) {
        await showActionError(label, error)
        return "error"
    }
}

function currentGitRepositoryMatches(repositoryRoot: string | null): boolean {
    const environment = useGitStore.getState().environment
    return environment?.status === "ready" && environment.root === repositoryRoot
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
export function worktreeCompareTarget(path: string): { files: WorktreeDiffFile[]; activePath: string } | null {
    const status = useGitStore.getState().status
    if (!status) return null

    const rel = relativeToRepoRoot(path)
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
function relativeToWorkspace(absPath: string, workspacePath: string | null): string {
    if (workspacePath && absPath.startsWith(workspacePath + "/")) {
        return absPath.slice(workspacePath.length + 1)
    }
    return absPath
}

function selectedText(view: EditorView): string {
    return view.state.selection.ranges
        .filter((range) => !range.empty)
        .map((range) => view.state.sliceDoc(range.from, range.to))
        .join("\n")
}

async function copySelection(view: EditorView): Promise<boolean> {
    const text = selectedText(view)
    if (!text) return false
    await writeText(text)
    return true
}

async function cutSelection(view: EditorView): Promise<boolean> {
    const text = selectedText(view)
    if (!text) return false
    await writeText(text)
    view.dispatch(view.state.replaceSelection(""))
    return true
}

async function pasteIntoEditor(view: EditorView): Promise<boolean> {
    const text = await readText()
    if (!text) return false
    view.dispatch(view.state.replaceSelection(text))
    return true
}

function editorTarget(
    request: Extract<ContextMenuRequest, { kind: "editor" }>
): RegisteredEditorView | null {
    const workspace = useWorkspaceStore.getState()
    if (workspace.workspacePath !== request.workspacePath) return null
    if (workspace.groups[request.groupIndex]?.activePath !== request.path) return null
    const entry = getViewEntry(request.path)
    return entry?.groupIndex === request.groupIndex ? entry : null
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
async function closeTabWithConfirm(groupIndex: number, path: string): Promise<ContextMenuCommandOutcome> {
    const group = useWorkspaceStore.getState().groups[groupIndex]
    const tab = group?.tabs.find((t) => t.path === path)
    if (!tab) return CONTEXT_MENU_CANCELLED
    if (tab.kind === "preview") {
        useWorkspaceStore.getState().closePreviewTab()
        return CONTEXT_MENU_COMPLETED
    }
    if (tab.dirty) {
        const ok = await confirm(i18n.t("contextMenu.confirm.closeDirtyTab", {
            ns: "menus",
            name: tab.path.split("/").pop() ?? tab.path
        }))
        if (!ok) return CONTEXT_MENU_CANCELLED
    }
    useWorkspaceStore.getState().closeTab(groupIndex, path)
    dropTabSideEffects(tab)
    return CONTEXT_MENU_COMPLETED
}

// "Close others" / "Close all": one combined confirm when any target tab is
// dirty (rather than TabBar's per-tab prompt) — closing a batch of tabs one
// dialog at a time would be tedious; declining leaves every tab open.
async function closeOtherTabsWithConfirm(groupIndex: number, keepPath: string): Promise<ContextMenuCommandOutcome> {
    const group = useWorkspaceStore.getState().groups[groupIndex]
    if (!group) return CONTEXT_MENU_CANCELLED
    const toClose = group.tabs.filter((t) => t.path !== keepPath)
    if (toClose.length === 0) return CONTEXT_MENU_CANCELLED
    if (toClose.some((t) => t.kind !== "preview" && t.dirty)) {
        const ok = await confirm(i18n.t("contextMenu.confirm.closeDirtyBatch", { ns: "menus" }))
        if (!ok) return CONTEXT_MENU_CANCELLED
    }
    useWorkspaceStore.getState().closeOtherTabs(groupIndex, keepPath)
    for (const t of toClose) {
        if (t.kind !== "preview") dropTabSideEffects(t)
    }
    return CONTEXT_MENU_COMPLETED
}

async function closeAllTabsWithConfirm(groupIndex: number): Promise<ContextMenuCommandOutcome> {
    const group = useWorkspaceStore.getState().groups[groupIndex]
    if (!group || group.tabs.length === 0) return CONTEXT_MENU_CANCELLED
    if (group.tabs.some((t) => t.kind !== "preview" && t.dirty)) {
        const ok = await confirm(i18n.t("contextMenu.confirm.closeDirtyBatch", { ns: "menus" }))
        if (!ok) return CONTEXT_MENU_CANCELLED
    }
    const tabs = group.tabs
    useWorkspaceStore.getState().closeAllTabs(groupIndex)
    for (const t of tabs) {
        if (t.kind !== "preview") dropTabSideEffects(t)
    }
    return CONTEXT_MENU_COMPLETED
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

async function createEntry(kind: "file" | "folder", workspace: string): Promise<ContextMenuCommandOutcome> {
    const ws = useWorkspaceStore.getState()
    if (ws.workspacePath !== workspace) return CONTEXT_MENU_CANCELLED
    const name = window.prompt(i18n.t(
        kind === "file" ? "contextMenu.prompt.newFile" : "contextMenu.prompt.newFolder",
        { ns: "menus" }
    ))?.trim()
    if (!name) return CONTEXT_MENU_CANCELLED
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
        return CONTEXT_MENU_COMPLETED
    } catch (e) {
        await message(String(e), {
            title: i18n.t("contextMenu.actionErrorTitle.create", { ns: "menus" }),
            kind: "error"
        })
        return CONTEXT_MENU_CANCELLED
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

async function renameEntry(path: string, workspace: string): Promise<ContextMenuCommandOutcome> {
    if (useWorkspaceStore.getState().workspacePath !== workspace) return CONTEXT_MENU_CANCELLED
    const currentName = path.split("/").pop() ?? path
    const name = window.prompt(
        i18n.t("contextMenu.prompt.rename", { ns: "menus" }),
        currentName
    )?.trim()
    if (!name || name === currentName) return CONTEXT_MENU_CANCELLED
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
        return CONTEXT_MENU_COMPLETED
    } catch (e) {
        await message(String(e), {
            title: i18n.t("contextMenu.actionErrorTitle.rename", { ns: "menus" }),
            kind: "error"
        })
        return CONTEXT_MENU_CANCELLED
    }
}

async function deleteEntry(path: string, isDir: boolean, workspace: string): Promise<ContextMenuCommandOutcome> {
    if (useWorkspaceStore.getState().workspacePath !== workspace) return CONTEXT_MENU_CANCELLED
    const name = path.split("/").pop() ?? path
    // The delete confirm below already covers the destructive intent (the file's
    // content — dirty buffer included — is going away regardless), so no separate
    // per-tab dirty prompt: it would be redundant once the user confirms delete.
    const text = i18n.t(
        isDir ? "contextMenu.confirm.deleteFolder" : "contextMenu.confirm.deleteFile",
        { ns: "menus", name }
    )
    const ok = await confirm(text, {
        title: i18n.t("contextMenu.confirm.deleteTitle", { ns: "menus" }),
        kind: "warning"
    })
    if (!ok) return CONTEXT_MENU_CANCELLED
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
        return CONTEXT_MENU_COMPLETED
    } catch (e) {
        await message(String(e), {
            title: i18n.t("contextMenu.actionErrorTitle.delete", { ns: "menus" }),
            kind: "error"
        })
        return CONTEXT_MENU_CANCELLED
    }
}

// cmOpenInBrowser (P3): serve the HTML file's own directory over a local http
// origin (so relative css/js/module specifiers resolve) and open that URL in the
// singleton preview tab. Same dir reuses one server/port on the Rust side.
async function openInBrowser(path: string, workspace: string): Promise<ContextMenuCommandOutcome> {
    if (useWorkspaceStore.getState().workspacePath !== workspace) return CONTEXT_MENU_CANCELLED
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
        return CONTEXT_MENU_COMPLETED
    } catch (e) {
        await message(String(e), {
            title: i18n.t("contextMenu.actionErrorTitle.preview", { ns: "menus" }),
            kind: "error"
        })
        return CONTEXT_MENU_CANCELLED
    }
}

// Target-specific legacy adapter for commands that still share existing domain
// operations. The registry owns visibility, availability, and dispatch.
export async function executeLegacyContextMenuAction(
    request: ContextMenuRequest,
    actionId: string
): Promise<ContextMenuCommandOutcome> {
    if (request.kind === "editor" && actionId === "cmCompareHead") {
        if (!editorTarget(request)) return CONTEXT_MENU_CANCELLED
        const target = worktreeCompareTarget(request.path)
        if (!target) return CONTEXT_MENU_CANCELLED
        useUiStore.getState().setMode("git")
        useDiffModalStore.getState().openWorktree(target.files, target.activePath)
        return CONTEXT_MENU_COMPLETED
    }

    if (actionId === "cmSettings") {
        useUiStore.getState().openSettings()
        return CONTEXT_MENU_COMPLETED
    }
    if (actionId === "cmHideSidebar") {
        useUiStore.getState().requestSidebarToggle()
        return CONTEXT_MENU_COMPLETED
    }
    if (actionId === "cmCmdPalette") {
        useUiStore.getState().requestOpenPalette()
        return CONTEXT_MENU_COMPLETED
    }

    if (request.kind === "tab" && actionId === "cmCloseTab") {
        return closeTabWithConfirm(request.groupIndex, request.path)
    }
    if (request.kind === "tab" && actionId === "cmCloseOthers") {
        return closeOtherTabsWithConfirm(request.groupIndex, request.path)
    }
    if (request.kind === "tab" && actionId === "cmCloseAll") {
        return closeAllTabsWithConfirm(request.groupIndex)
    }
    if (request.kind === "tab" && actionId === "cmSplit") {
        useWorkspaceStore.getState().splitAndMoveRight(request.groupIndex, request.path)
        return CONTEXT_MENU_COMPLETED
    }

    if ((request.kind === "tab" || request.kind === "file") && actionId === "cmCopyRel") {
        await writeText(relativeToWorkspace(request.path, request.workspacePath))
        return CONTEXT_MENU_COMPLETED
    }

    if (request.kind === "file" && actionId === "cmOpen") {
        useWorkspaceStore.getState().openTab(request.path, request.sourceGroupIndex)
        return CONTEXT_MENU_COMPLETED
    }
    if (request.kind === "file" && actionId === "cmOpenSplit") {
        useWorkspaceStore.getState().openInRightSplit(request.path, request.sourceGroupIndex)
        return CONTEXT_MENU_COMPLETED
    }
    if (request.kind === "file" && actionId === "cmRename") {
        return renameEntry(request.path, request.workspacePath)
    }
    if (request.kind === "file" && actionId === "cmDelete") {
        return deleteEntry(request.path, request.isDirectory, request.workspacePath)
    }
    if (request.kind === "file" && actionId === "cmReveal") {
        await revealItemInDir(request.path)
        return CONTEXT_MENU_COMPLETED
    }
    if (request.kind === "file" && actionId === "cmOpenInBrowser") {
        return openInBrowser(request.path, request.workspacePath)
    }

    if (request.kind === "explorer" && request.workspacePath) {
        if (actionId === "cmCopyPath") {
            await writeText(request.workspacePath)
            return CONTEXT_MENU_COMPLETED
        }
        if (actionId === "cmNewFile") return createEntry("file", request.workspacePath)
        if (actionId === "cmNewFolder") return createEntry("folder", request.workspacePath)
    }

    if (request.kind === "editor" && (actionId === "cmCut" || actionId === "cmCopy" || actionId === "cmPaste")) {
        const target = editorTarget(request)
        if (!target) return CONTEXT_MENU_CANCELLED
        const changed = actionId === "cmCopy"
            ? await copySelection(target.view)
            : actionId === "cmCut"
              ? await cutSelection(target.view)
              : await pasteIntoEditor(target.view)
        return changed ? CONTEXT_MENU_COMPLETED : CONTEXT_MENU_CANCELLED
    }
    if (request.kind === "editor" && actionId === "cmFormatDoc") {
        const target = editorTarget(request)
        if (!target?.formatDocument) return CONTEXT_MENU_CANCELLED
        return (await target.formatDocument()) ? CONTEXT_MENU_COMPLETED : CONTEXT_MENU_CANCELLED
    }

    if (
        (request.kind === "git" || request.kind === "status") &&
        currentGitRepositoryMatches(request.repositoryRoot) &&
        actionId === "cmFetch"
    ) {
        if (!request.repositoryRoot) return CONTEXT_MENU_CANCELLED
        return (await useGitStore.getState().runOp(
            "fetch",
            () => gitFetch(false, request.repositoryRoot!)
        ))
            ? CONTEXT_MENU_COMPLETED
            : CONTEXT_MENU_CANCELLED
    }
    if (
        (request.kind === "git" || request.kind === "status") &&
        currentGitRepositoryMatches(request.repositoryRoot) &&
        actionId === "cmPull"
    ) {
        if (!request.repositoryRoot) return CONTEXT_MENU_CANCELLED
        return (await useGitStore.getState().runOp("pull", () => gitPull(request.repositoryRoot!)))
            ? CONTEXT_MENU_COMPLETED
            : CONTEXT_MENU_CANCELLED
    }
    if (
        (request.kind === "git" || request.kind === "status") &&
        currentGitRepositoryMatches(request.repositoryRoot) &&
        actionId === "cmPush"
    ) {
        if (!request.repositoryRoot) return CONTEXT_MENU_CANCELLED
        return (await useGitStore.getState().runOp("push", () => gitPush(request.repositoryRoot!)))
            ? CONTEXT_MENU_COMPLETED
            : CONTEXT_MENU_CANCELLED
    }
    if (
        (request.kind === "git" || request.kind === "status") &&
        currentGitRepositoryMatches(request.repositoryRoot) &&
        actionId === "cmCopyBranch"
    ) {
        const branch = useGitStore.getState().status?.branch
        if (!branch) return CONTEXT_MENU_CANCELLED
        await writeText(branch)
        return CONTEXT_MENU_COMPLETED
    }
    if (
        (request.kind === "git" || request.kind === "status") &&
        currentGitRepositoryMatches(request.repositoryRoot) &&
        actionId === "cmCopyHash"
    ) {
        const headOid = useGitStore.getState().status?.headOid
        if (!headOid) return CONTEXT_MENU_CANCELLED
        await writeText(headOid)
        return CONTEXT_MENU_COMPLETED
    }

    if (request.kind === "sshhost" && actionId === "cmCopyAddr") {
        const host = useSshStore.getState().hosts.find((candidate) => candidate.id === request.hostId)
        if (!host) return CONTEXT_MENU_CANCELLED
        await writeText(`${host.user}@${host.host}:${host.port}`)
        return CONTEXT_MENU_COMPLETED
    }
    if (request.kind === "sshhost" && actionId === "cmOpenSsh") {
        const state = useSshStore.getState()
        if (
            !state.hosts.some((host) => host.id === request.hostId) ||
            state.pendingAuthHostId === request.hostId ||
            state.sessions[request.hostId]?.status === "connecting"
        ) {
            return CONTEXT_MENU_CANCELLED
        }
        state.beginConnect(request.hostId)
        return CONTEXT_MENU_COMPLETED
    }
    if (request.kind === "sshhost" && actionId === "cmDisconnect") {
        const state = useSshStore.getState()
        if (!state.hosts.some((host) => host.id === request.hostId)) return CONTEXT_MENU_CANCELLED
        const session = state.sessions[request.hostId]
        if (session?.status !== "connected" || !session.sessionId) return CONTEXT_MENU_CANCELLED
        await state.disconnect(request.hostId)
        return CONTEXT_MENU_COMPLETED
    }
    if (request.kind === "sshhost" && actionId === "cmOpenSftp") {
        const state = useSshStore.getState()
        if (
            !state.hosts.some((host) => host.id === request.hostId) ||
            state.pendingAuthHostId === request.hostId ||
            state.sessions[request.hostId]?.status === "connecting"
        ) {
            return CONTEXT_MENU_CANCELLED
        }
        useSftpStore.getState().openSftp(request.hostId)
        return CONTEXT_MENU_COMPLETED
    }

    if (request.kind === "dbconn" && actionId === "cmCopyAddr") {
        const descriptor = useDbStore.getState().saved.find(
            (candidate) => candidate.id === request.descriptorId
        )
        if (!descriptor) return CONTEXT_MENU_CANCELLED
        await writeText(savedConnectionAddress(descriptor))
        return CONTEXT_MENU_COMPLETED
    }
    if (request.kind === "dbconn" && actionId === "cmDisconnect") {
        const disconnected = await useDbStore.getState().disconnect(request.descriptorId)
        if (!disconnected) {
            const errorCode = useDbStore.getState().sessions[request.descriptorId]?.error ?? "unknown"
            throw new Error(i18n.t(`database.profileError.${errorCode}`, { ns: "workbench" }))
        }
        return CONTEXT_MENU_COMPLETED
    }

    return CONTEXT_MENU_CANCELLED
}

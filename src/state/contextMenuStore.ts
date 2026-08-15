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
import {
    isSameOrDescendantPath,
    nativePathJoin,
    rebasePath,
    relativePathWithin,
    workspacePathBasename,
} from "@/lib/paths"
import { dropDocument, renameDocument } from "../editor/documentRegistry"
import { getView, getViewEntry, type RegisteredEditorView } from "../editor/viewRegistry"
import { logUserAction } from "@/features/logs/userAction"
import { showActionError } from "@/lib/actionFeedback"
import { requestAppConfirmation, showAppMessage } from "@/state/appDialogStore"
import { requestTextInputDialog } from "@/state/textInputDialogStore"
import {
    fsCreateDir,
    fsCreateFile,
    fsDelete,
    fsRename,
    gitFetch,
    gitPull,
    gitPush,
    previewCreate,
    previewRevoke
} from "../lib/ipc"
import { isFileTab, isMarkdownPreviewTab } from "../lib/markdownPreviewTab"
import { usePreviewStore } from "./previewStore"
import { useSvgPreviewStore } from "./svgPreviewStore"
import { worktreeFilesFrom } from "../workbench/git/fileRows"
import { useDiffModalStore, type WorktreeDiffFile } from "./diffModalStore"
import { useFileTreeStore } from "./fileTreeStore"
import { useGitStore } from "./gitStore"
import { savedConnectionAddress, useDbStore } from "./dbStore"
import { useSftpStore } from "./sftpStore"
import { useSshStore } from "./sshStore"
import { useUiStore } from "./uiStore"
import { useWorkspaceStore, type TabInfo } from "./workspaceStore"

export type { ContextMenuRequest } from "@/app/workbench/contextMenuModel"

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
    return relativePathWithin(environment.root, activePath)
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
    if (!workspacePath) return absPath
    return relativePathWithin(workspacePath, absPath) ?? absPath
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
    if (isFileTab(tab)) dropDocument(tab.path)
    // Reopening an SVG returns to the default-open preview state (its store
    // records explicit closes, so dropping the flag restores the default).
    useSvgPreviewStore.getState().forget(tab.path)
}

// Single-tab close, replicating TabBar's onClose confirm flow so the tab
// context menu's "Close tab" behaves identically to clicking the tab's own
// close button.
async function closeTabWithConfirm(groupIndex: number, path: string): Promise<ContextMenuCommandOutcome> {
    const group = useWorkspaceStore.getState().groups[groupIndex]
    const tab = group?.tabs.find((t) => t.path === path)
    if (!tab || tab.kind === "herdr-terminal") return CONTEXT_MENU_CANCELLED
    if (tab.kind === "preview") {
        useWorkspaceStore.getState().closePreviewTab()
        return CONTEXT_MENU_COMPLETED
    }
    if (isMarkdownPreviewTab(tab)) {
        useWorkspaceStore.getState().closeMarkdownPreviewTab(groupIndex, path)
        return CONTEXT_MENU_COMPLETED
    }
    if (tab.dirty) {
        const ok = await requestAppConfirmation({
            title: i18n.t("contextMenu.confirm.closeDirtyTitle", { ns: "menus" }),
            description: i18n.t("contextMenu.confirm.closeDirtyTab", {
                ns: "menus",
                name: workspacePathBasename(tab.path)
            }),
            kind: "warning",
            destructive: true
        })
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
    // Generic file-tab batch actions must never silently remove runtime-backed
    // Herdr pages. Those require their explicit destructive Herdr close flow.
    const toClose = group.tabs.filter(
        (tab) => tab.path !== keepPath && tab.kind !== "herdr-terminal"
    )
    if (toClose.length === 0) return CONTEXT_MENU_CANCELLED
    if (toClose.some((t) => isFileTab(t) && t.dirty)) {
        const ok = await requestAppConfirmation({
            title: i18n.t("contextMenu.confirm.closeDirtyTitle", { ns: "menus" }),
            description: i18n.t("contextMenu.confirm.closeDirtyBatch", { ns: "menus" }),
            kind: "warning",
            destructive: true
        })
        if (!ok) return CONTEXT_MENU_CANCELLED
    }
    useWorkspaceStore.getState().closeTabsByPath(toClose.map((tab) => tab.path))
    for (const t of toClose) {
        if (isFileTab(t)) dropTabSideEffects(t)
    }
    return CONTEXT_MENU_COMPLETED
}

async function closeAllTabsWithConfirm(groupIndex: number): Promise<ContextMenuCommandOutcome> {
    const group = useWorkspaceStore.getState().groups[groupIndex]
    if (!group || group.tabs.length === 0) return CONTEXT_MENU_CANCELLED
    const tabs = group.tabs.filter((tab) => tab.kind !== "herdr-terminal")
    if (tabs.length === 0) return CONTEXT_MENU_CANCELLED
    if (tabs.some((t) => isFileTab(t) && t.dirty)) {
        const ok = await requestAppConfirmation({
            title: i18n.t("contextMenu.confirm.closeDirtyTitle", { ns: "menus" }),
            description: i18n.t("contextMenu.confirm.closeDirtyBatch", { ns: "menus" }),
            kind: "warning",
            destructive: true
        })
        if (!ok) return CONTEXT_MENU_CANCELLED
    }
    useWorkspaceStore.getState().closeTabsByPath(tabs.map((tab) => tab.path))
    for (const t of tabs) {
        if (isFileTab(t)) dropTabSideEffects(t)
    }
    return CONTEXT_MENU_COMPLETED
}

// --- explorer/file: filesystem operations (PROB-5 後波) ---
// Every op validates the workspace boundary in Rust; failures are
// surfaced through a dialog message rather than swallowed. On success we run a
// precise file-tree invalidation (#59 T4b: re-list only the affected cached
// dirs; it also bumps treeRevision for mention-index consumers — FileTree
// doesn't subscribe to the fs watcher for its own ops).
function joinName(dir: string, name: string): string {
    return nativePathJoin(dir, name)
}

async function createEntry(kind: "file" | "folder", workspace: string): Promise<ContextMenuCommandOutcome> {
    const ws = useWorkspaceStore.getState()
    if (ws.workspacePath !== workspace) return CONTEXT_MENU_CANCELLED
    const prompt = i18n.t(
        kind === "file" ? "contextMenu.prompt.newFile" : "contextMenu.prompt.newFolder",
        { ns: "menus" }
    )
    const name = await requestTextInputDialog({
        title: prompt,
        label: i18n.t("textInputDialog.nameLabel", { ns: "menus" }),
        confirmLabel: i18n.t("textInputDialog.create", { ns: "menus" })
    })
    if (!name) return CONTEXT_MENU_CANCELLED
    const target = joinName(workspace, name)
    try {
        if (kind === "file") {
            await fsCreateFile(workspace, target)
            await useFileTreeStore.getState().invalidatePaths(workspace, [target])
            useWorkspaceStore.getState().openTab(target)
        } else {
            await fsCreateDir(workspace, target)
            await useFileTreeStore.getState().invalidatePaths(workspace, [target])
        }
        return CONTEXT_MENU_COMPLETED
    } catch (e) {
        await showAppMessage({
            title: i18n.t("contextMenu.actionErrorTitle.create", { ns: "menus" }),
            description: String(e),
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
            if (!isFileTab(t)) continue
            if (isSameOrDescendantPath(target, t.path)) paths.add(t.path)
        }
    }
    return [...paths]
}

async function renameEntry(path: string, workspace: string): Promise<ContextMenuCommandOutcome> {
    if (useWorkspaceStore.getState().workspacePath !== workspace) return CONTEXT_MENU_CANCELLED
    const currentName = workspacePathBasename(path)
    const name = await requestTextInputDialog({
        title: i18n.t("contextMenu.prompt.rename", { ns: "menus" }),
        label: i18n.t("textInputDialog.nameLabel", { ns: "menus" }),
        initialValue: currentName,
        confirmLabel: i18n.t("textInputDialog.rename", { ns: "menus" })
    })
    if (!name || name === currentName) return CONTEXT_MENU_CANCELLED
    const slash = Math.max(path.lastIndexOf("/"), path.lastIndexOf("\\"))
    const target = path.slice(0, slash + 1) + name
    try {
        await fsRename(workspace, path, target)
        // Re-point every open tab + its editor state from the old path to the new
        // one, so a later save lands on the renamed file instead of recreating the
        // old path (Finding 3). Move the document-registry entry (snapshotting the
        // live buffer so unsaved edits survive) before updateTabPath remounts
        // EditorPane. Linked markdown-preview tabs are store-owned adjacent-group
        // tabs; updateTabPath rebases their sourcePath/identity in the same pass.
        const svgPreview = useSvgPreviewStore.getState()
        for (const oldPath of affectedTabPaths(path)) {
            const newPath = rebasePath(path, target, oldPath)
            if (!newPath) continue
            renameDocument(oldPath, newPath, getView(oldPath)?.state.doc.toString())
            // SVG previews default open, so the *closed* flag is what follows
            // the rename. Forget the old path unconditionally: after a double-toggle
            // the store holds a false-valued flag (isOpen already true) that would
            // otherwise linger on the stale path until a workspace switch.
            const svgWasClosed = !svgPreview.isOpen(oldPath)
            svgPreview.forget(oldPath)
            if (svgWasClosed && svgPreview.isOpen(newPath)) svgPreview.toggle(newPath)
        }
        useWorkspaceStore.getState().updateTabPath(path, target)
        // 舊、新路徑一起失效：同層 re-list 反映改名；資料夾改名時 relist 的
        // prune 會丟掉舊路徑底下的快取子樹（新路徑首次展開時重新 list）。
        await useFileTreeStore.getState().invalidatePaths(workspace, [path, target])
        return CONTEXT_MENU_COMPLETED
    } catch (e) {
        await showAppMessage({
            title: i18n.t("contextMenu.actionErrorTitle.rename", { ns: "menus" }),
            description: String(e),
            kind: "error"
        })
        return CONTEXT_MENU_CANCELLED
    }
}

async function deleteEntry(path: string, isDir: boolean, workspace: string): Promise<ContextMenuCommandOutcome> {
    if (useWorkspaceStore.getState().workspacePath !== workspace) return CONTEXT_MENU_CANCELLED
    const name = workspacePathBasename(path)
    // The delete confirm below already covers the destructive intent (the file's
    // content — dirty buffer included — is going away regardless), so no separate
    // per-tab dirty prompt: it would be redundant once the user confirms delete.
    const text = i18n.t(
        isDir ? "contextMenu.confirm.deleteFolder" : "contextMenu.confirm.deleteFile",
        { ns: "menus", name }
    )
    const ok = await requestAppConfirmation({
        title: i18n.t("contextMenu.confirm.deleteTitle", { ns: "menus" }),
        description: text,
        kind: "warning",
        destructive: true
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
                useSvgPreviewStore.getState().forget(p)
            }
        }
        await useFileTreeStore.getState().invalidatePaths(workspace, [path])
        return CONTEXT_MENU_COMPLETED
    } catch (e) {
        await showAppMessage({
            title: i18n.t("contextMenu.actionErrorTitle.delete", { ns: "menus" }),
            description: String(e),
            kind: "error"
        })
        return CONTEXT_MENU_CANCELLED
    }
}

// cmOpenInBrowser (P3): create a revocable per-document static preview session
// and open that isolated URL in the singleton preview tab.
async function openInBrowser(path: string, workspace: string): Promise<ContextMenuCommandOutcome> {
    if (useWorkspaceStore.getState().workspacePath !== workspace) return CONTEXT_MENU_CANCELLED
    try {
        const session = await previewCreate(path)
        if (useWorkspaceStore.getState().workspacePath !== workspace) {
            void previewRevoke(session.token).catch(() => undefined)
            return CONTEXT_MENU_CANCELLED
        }
        useWorkspaceStore.getState().openPreviewTab()
        usePreviewStore.getState().openStaticPreview(workspace, session)
        return CONTEXT_MENU_COMPLETED
    } catch (e) {
        await showAppMessage({
            title: i18n.t("contextMenu.actionErrorTitle.preview", { ns: "menus" }),
            description: staticPreviewErrorMessage(e),
            kind: "error"
        })
        return CONTEXT_MENU_CANCELLED
    }
}

function staticPreviewErrorMessage(error: unknown): string {
    const raw = String(error)
    if (raw.includes("preview-graph-too-large")) {
        return i18n.t("staticCreateTooLarge", { ns: "preview" })
    }
    return i18n.t("staticCreateFailed", { ns: "preview" })
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
        const environment = useGitStore.getState().environment
        if (environment?.status !== "ready") return CONTEXT_MENU_CANCELLED
        useUiStore.getState().setMode("git")
        useDiffModalStore.getState().openWorktree(environment.root, target.files, target.activePath)
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
            () => gitFetch(request.repositoryRoot!, false)
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

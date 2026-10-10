import { gitFileNameStyle, worktreeFilesFrom, worktreeFileMetadata } from "./git/fileRows"
import { ChevronDown, ChevronRight, GitCompareArrows } from "lucide-react"
import { type KeyboardEvent, type MouseEvent, type PointerEvent, useEffect, useMemo, useRef } from "react"
import { useTranslation } from "react-i18next"
import { logUserAction } from "@/features/logs/userAction"
import { canonicalPathKey, isSameOrDescendantPath, nativePathParent, relativePathWithin, workspacePathBasename } from "@/lib/paths"
import { beginPointerDrag, elementAtPoint, type DragPoint, type PointerDropTarget } from "@/lib/pointerDrag"
import { remoteWorkspaceCanMove } from "@/lib/remoteFiles"
import { parseRemoteFilePath } from "@/lib/runtimeIdentity"
import { requestAppConfirmation } from "@/state/appDialogStore"
import { terminalDropTargetAt, type TerminalDropTarget } from "@/terminal/terminalDropTargets"
import { notifyTerminalPathPasteError, pastePathsIntoTerminal } from "@/terminal/terminalPathPaste"
import { FileIcon } from "../lib/fileIcons"
import type { FileNode, GitStatus } from "../lib/types"
import { contextMenuHandler } from "../state/contextMenuStore"
import { useFileTreeStore } from "../state/fileTreeStore"
import { useGitStore } from "../state/gitStore"
import { useDiffModalStore } from "../state/diffModalStore"
import { useWorkspaceStore } from "../state/workspaceStore"
import { useFileClipboardStore } from "../state/fileClipboardStore"
import { isMacPlatform } from "@/lib/platform"
import { copyFilesToClipboard, duplicatePath, moveFilesTo, pasteFiles } from "./fileClipboard"
import { containingFolderRow } from "./fileTreeDom"

// Repo-relative form of an absolute node path, matched against the git status
// (which reports paths relative to the repo root). Uses forward slashes.
function relativePath(path: string, root: string | null) {
    if (!root) return path
    return relativePathWithin(root, path) ?? path
}

type TreeDragData =
    | { kind: "terminal"; terminal: TerminalDropTarget }
    | { kind: "move"; dir: string }
    | { kind: "open"; group: number }

// Whether moving `source` into `dir` would change anything.
function canMoveInto(source: string, dir: string) {
    if (isSameOrDescendantPath(source, dir)) return false
    try {
        return canonicalPathKey(nativePathParent(source)) !== canonicalPathKey(dir)
    } catch {
        return false
    }
}

// Controlled node (#59 T4b): expansion + children live in fileTreeStore's
// per-workspace bucket instead of component state, so they survive workspace
// switches and precise invalidations never remount the tree.
function TreeNode({ node, root, depth }: { node: FileNode; root: string; depth: number }) {
    const { t } = useTranslation("menus")
    const expanded = useFileTreeStore(
        (s) => node.isDir && (s.trees[root]?.expandedDirs.has(node.path) ?? false)
    )
    const children = useFileTreeStore((s) =>
        node.isDir ? s.trees[root]?.childrenByDir[node.path] ?? null : null
    )
    const openTab = useWorkspaceStore((s) => s.openTab)
    const workspacePath = useWorkspaceStore((s) => s.workspacePath)
    const sourceGroupIndex = useWorkspaceStore((s) => s.activeGroupIndex)
    const active = useWorkspaceStore(
        (s) => !node.isDir && s.groups[s.activeGroupIndex]?.activePath === node.path
    )
    // git status paths are relative to the repo root, which may sit above the
    // opened workspace (workspace = repo subdirectory). Use environment.root when
    // ready; fall back to workspacePath otherwise.
    // Subscribe only to the two values displayed by this row. The first
    // character carries staged state; the remainder is the full status code.
    // Diff actions below read the current metadata directly from the store.
    const selectGitFileState = useMemo(() => {
        if (node.isDir) return () => null
        let previousStatus: GitStatus | null | undefined
        let previousRoot: string | null | undefined
        let previousRelativePath = ""
        let value: string | null = null
        return (s: ReturnType<typeof useGitStore.getState>) => {
            const repoRoot = s.environment?.status === "ready" ? s.environment.root : workspacePath
            if (s.status === previousStatus && repoRoot === previousRoot) return value
            const rel = repoRoot === previousRoot ? previousRelativePath : relativePath(node.path, repoRoot)
            const file = worktreeFileMetadata(s.status).get(rel)
            value = file ? `${file.staged ? "1" : "0"}${file.status}` : null
            previousStatus = s.status
            previousRoot = repoRoot
            previousRelativePath = rel
            return value
        }
    }, [node.isDir, node.path, workspacePath])
    const gitFileState = useGitStore(selectGitFileState)
    const isChanged = gitFileState !== null
    const selected = useFileClipboardStore(
        (s) => s.selection?.workspacePath === root && s.selection.path === node.path
    )
    const cut = useFileClipboardStore(
        (s) => s.clipboard?.mode === "cut" && s.clipboard.workspacePath === root && s.clipboard.paths.includes(node.path)
    )

    function onClick(event: MouseEvent<HTMLButtonElement>) {
        // WebKit (macOS) does not focus a clicked button; without focus the
        // tree's Cmd+C / Cmd+V never fire and the menu copies page text instead.
        event.currentTarget.focus({ preventScroll: true })
        useFileClipboardStore.getState().select(root, node.path)
        if (node.isDir) {
            void useFileTreeStore.getState().toggleDir(root, node.path)
        } else {
            // Single click previews the file in the group's reusable preview-mode tab.
            openTab(node.path, undefined, { transient: true })
            void logUserAction("open_file", `open ${node.path}`)
        }
    }

    const disposeDrag = useRef<(() => void) | null>(null)
    useEffect(() => () => disposeDrag.current?.(), [])

    function resolveDragTarget(point: DragPoint): PointerDropTarget<TreeDragData> | null {
        const hit = elementAtPoint(point)
        const terminal = terminalDropTargetAt(hit)
        const leaf = hit?.closest<HTMLElement>("[data-attachment-key]")
        if (terminal && leaf) return { element: leaf, data: { kind: "terminal", terminal } }
        if (useWorkspaceStore.getState().workspacePath !== root) return null
        const zone = hit?.closest<HTMLElement>("[data-file-tree-root]")
        const row = hit?.closest<HTMLElement>("[data-tree-path]")
        if (row || zone) {
            // SFTP has no in-place move; only runtime-backed remote workspaces do.
            if (parseRemoteFilePath(root) && !remoteWorkspaceCanMove(root)) return null
            // A file row stands for the folder that holds it.
            const folder = row?.dataset.treeDir === "true" ? row : row ? containingFolderRow(row) : null
            const dir = folder?.dataset.treePath ?? root
            const element = folder ?? zone
            if (!element || !isSameOrDescendantPath(root, dir) || !canMoveInto(node.path, dir)) return null
            return { element, data: { kind: "move", dir } }
        }
        const group = hit?.closest<HTMLElement>("[data-editor-group-index]")
        const index = Number(group?.dataset.editorGroupIndex)
        if (group && !node.isDir && Number.isInteger(index)) return { element: group, data: { kind: "open", group: index } }
        return null
    }

    async function dropOn({ data }: PointerDropTarget<TreeDragData>) {
        if (data.kind === "terminal") {
            await pastePathsIntoTerminal(data.terminal, [node.path]).catch(notifyTerminalPathPasteError)
            return
        }
        if (useWorkspaceStore.getState().workspacePath !== root) return
        if (data.kind === "open") {
            useWorkspaceStore.getState().openTabInGroup(node.path, data.group)
            return
        }
        const accepted = await requestAppConfirmation({
            title: t("fileTree.moveConfirm.title"),
            description: t("fileTree.moveConfirm.description", {
                name: node.name,
                target: workspacePathBasename(data.dir)
            }),
            confirmLabel: t("fileTree.moveConfirm.confirm"),
            kind: "warning"
        })
        if (accepted) await moveFilesTo(root, [node.path], data.dir)
    }

    function onPointerDown(event: PointerEvent<HTMLButtonElement>) {
        const row = event.currentTarget
        disposeDrag.current = beginPointerDrag<TreeDragData>(event, {
            label: node.name,
            autoScroll: () => [
                row.closest('[data-slot="scroll-area-viewport"], [data-radix-scroll-area-viewport]')
            ],
            resolveTarget: resolveDragTarget,
            onDrop: (target) => void dropOn(target)
        })
    }

    function onDoubleClick() {
        // Double click keeps the file open as a regular tab.
        if (!node.isDir) openTab(node.path)
    }

    return (
        <li>
            <div className="group relative">
                <button
                    type="button"
                    onClick={onClick}
                    onPointerDown={onPointerDown}
                    onDoubleClick={onDoubleClick}
                    data-tree-path={node.path}
                    data-pointer-drag-handle="pan-y"
                    data-tree-dir={node.isDir ? "true" : undefined}
                    data-selected={selected ? "true" : undefined}
                    onContextMenu={workspacePath ? (event) => {
                        useFileClipboardStore.getState().select(root, node.path)
                        contextMenuHandler({
                            kind: "file",
                            workspacePath,
                            path: node.path,
                            isDirectory: node.isDir,
                            sourceGroupIndex
                        })(event)
                    } : undefined}
                    style={{ paddingLeft: `${14 + depth * 15}px` }}
                    className={
                        "flex h-[27px] w-full items-center gap-[7px] rounded-[8px] pr-[8px] text-left text-[12.5px] transition-colors duration-100 " +
                        (active
                            ? "bg-(--yz-active) text-(--ink-0) shadow-(--shadow-xs)"
                            : selected
                              ? "bg-(--yz-hover) hover:bg-(--yz-hover)"
                              : "hover:bg-(--yz-hover)") +
                        (cut ? " opacity-55" : "") +
                        " data-[pointer-drop-target=inside]:bg-(--yz-hover) data-[pointer-drop-target=inside]:shadow-[inset_0_0_0_1.5px_var(--yz-accent)]"
                    }
                >
                    {node.isDir ? (
                        <>
                            {expanded ? (
                                <ChevronDown className="size-[13px] shrink-0 text-(--ink-3)" aria-hidden="true" />
                            ) : (
                                <ChevronRight className="size-[13px] shrink-0 text-(--ink-3)" aria-hidden="true" />
                            )}
                            <FileIcon
                                fileName={node.name}
                                isDirectory
                                isOpen={expanded}
                                className="size-[16px] shrink-0"
                            />
                        </>
                    ) : (
                        <FileIcon
                            fileName={node.name}
                            className={"size-[16px] shrink-0" + (active ? "" : " opacity-85")}
                        />
                    )}
                    <span
                        style={gitFileState !== null ? gitFileNameStyle(gitFileState.slice(1), gitFileState[0] === "1") : undefined}
                        className={
                            "truncate " +
                            (node.isDir
                                ? "font-semibold text-(--ink-1)"
                                : active
                                  ? "font-medium"
                                  : "font-normal text-(--ink-2)")
                        }
                    >
                        {node.name}
                    </span>
                </button>
                {isChanged && (
                    <button
                        type="button"
                        aria-label={t("fileTree.openDiffFile", { name: node.name })}
                        title={t("fileTree.openDiffTitle")}
                        onClick={() => {
                            const git = useGitStore.getState()
                            if (git.environment?.status !== "ready") return
                            const rel = relativePath(node.path, git.environment.root)
                            const files = worktreeFilesFrom(git.status)
                            const file = files.find((entry) => entry.path === rel && !entry.staged) ?? files.find((entry) => entry.path === rel)
                            if (file) useDiffModalStore.getState().openWorktree(git.environment.root, files, { path: rel, staged: file.staged })
                        }}
                        className="absolute top-1/2 right-[6px] flex size-[20px] -translate-y-1/2 items-center justify-center rounded-[6px] text-(--ink-3) opacity-0 transition-[opacity,background-color,color] duration-[130ms] group-hover:opacity-100 group-focus-within:opacity-100 focus-visible:opacity-100 hover:bg-(--yz-hover) hover:text-(--yz-accent-ink)"
                    >
                        <GitCompareArrows className="size-[13px]" aria-hidden="true" />
                    </button>
                )}
            </div>
            {node.isDir && expanded && children !== null && (
                <ul>
                    {children.map((child) => (
                        <TreeNode key={child.path} node={child} root={root} depth={depth + 1} />
                    ))}
                </ul>
            )}
        </li>
    )
}

function runClipboardKey(workspacePath: string, key: string, row: HTMLElement) {
    const path = row.dataset.treePath
    if (!path) return
    useFileClipboardStore.getState().select(workspacePath, path)
    if (key === "c" || key === "x") void copyFilesToClipboard(workspacePath, [path], key === "x" ? "cut" : "copy")
    else if (key === "v") void pasteFiles(workspacePath, { path, isDirectory: row.dataset.treeDir === "true" })
    else void duplicatePath(workspacePath, path)
}

export function FileTree() {
    const workspacePath = useWorkspaceStore((s) => s.workspacePath)
    // treeRevision stays the shared invalidation authority (context-menu ops,
    // explorer "Refresh", external changes, AgentZone mention index) but no
    // longer remounts the tree: a bump either was already applied precisely by
    // fileTreeStore (marker consumed → skip) or triggers a background
    // revalidate that diff-applies without dropping expansion state.
    const treeRevision = useWorkspaceStore((s) => s.treeRevision)
    const rootNodes = useFileTreeStore((s) =>
        workspacePath ? s.trees[workspacePath]?.rootNodes ?? null : null
    )
    const listRef = useRef<HTMLUListElement | null>(null)
    const prevRootRef = useRef<string | null>(null)

    useEffect(() => {
        if (!workspacePath) {
            prevRootRef.current = null
            return
        }
        const switched = prevRootRef.current !== workspacePath
        prevRootRef.current = workspacePath
        const fileTree = useFileTreeStore.getState()
        // A workspace switch always revalidates (hydrate happens synchronously
        // from the store bucket; this refresh runs in the background).
        if (!switched && fileTree.consumePreciseRevision(workspacePath, treeRevision)) return
        void fileTree.ensureTree(workspacePath)
    }, [workspacePath, treeRevision])

    // Persist/restore the nav scroller offset per workspace. Prefer the nearest
    // ScrollArea viewport (Radix wraps children in an intermediate table div);
    // fall back to the immediate parent for unit tests that mount FileTree alone.
    useEffect(() => {
        if (!workspacePath) return
        const scroller =
            (listRef.current?.closest(
                '[data-slot="scroll-area-viewport"], [data-radix-scroll-area-viewport]'
            ) as HTMLElement | null) ?? listRef.current?.parentElement
        if (!scroller) return
        scroller.scrollTop = useFileTreeStore.getState().trees[workspacePath]?.scrollTop ?? 0
        const onScroll = () =>
            useFileTreeStore.getState().setScrollTop(workspacePath, scroller.scrollTop)
        scroller.addEventListener("scroll", onScroll, { passive: true })
        return () => scroller.removeEventListener("scroll", onScroll)
    }, [workspacePath])

    // macOS routes Cmd+C / Cmd+X / Cmd+V through the Edit menu, so the page
    // never sees those keydowns. WebKit fires clipboard events instead, and
    // only once a `before*` listener claims them; without a text selection the
    // menu items otherwise stay disabled.
    useEffect(() => {
        if (!workspacePath) return
        const focusedRow = () => {
            const active = document.activeElement
            return active instanceof HTMLElement && listRef.current?.contains(active)
                ? active.closest<HTMLElement>("[data-tree-path]")
                : null
        }
        // Blank tree space focuses the tree root (FilesNavContent); a paste there lands in the workspace root.
        const zone = listRef.current?.closest<HTMLElement>("[data-file-tree-root]") ?? null
        const blankFocused = () => zone !== null && document.activeElement === zone
        const claim = (event: Event) => {
            if (focusedRow() || (event.type === "beforepaste" && blankFocused())) event.preventDefault()
        }
        const act = (event: Event) => {
            const row = focusedRow()
            if (!row) {
                if (event.type === "paste" && blankFocused()) {
                    event.preventDefault()
                    void pasteFiles(workspacePath, null)
                }
                return
            }
            event.preventDefault()
            runClipboardKey(workspacePath, event.type === "copy" ? "c" : event.type === "cut" ? "x" : "v", row)
        }
        const claims = ["beforecopy", "beforecut", "beforepaste"]
        const actions = ["copy", "cut", "paste"]
        const onZoneKeyDown = (event: globalThis.KeyboardEvent) => {
            const mod = isMacPlatform() ? event.metaKey && !event.ctrlKey : event.ctrlKey && !event.metaKey
            if (event.target !== zone || !mod || event.altKey || event.shiftKey || event.key.toLowerCase() !== "v") return
            event.preventDefault()
            void pasteFiles(workspacePath, null)
        }
        claims.forEach((type) => document.addEventListener(type, claim))
        actions.forEach((type) => document.addEventListener(type, act))
        zone?.addEventListener("keydown", onZoneKeyDown)
        return () => {
            zone?.removeEventListener("keydown", onZoneKeyDown)
            claims.forEach((type) => document.removeEventListener(type, claim))
            actions.forEach((type) => document.removeEventListener(type, act))
        }
    }, [workspacePath])

    if (!workspacePath) return null

    // Finder / Explorer style clipboard keys while a tree row has focus.
    const onKeyDown = (event: KeyboardEvent<HTMLUListElement>) => {
        const mod = isMacPlatform() ? event.metaKey && !event.ctrlKey : event.ctrlKey && !event.metaKey
        if (!mod || event.altKey || event.shiftKey) return
        const row = (event.target as HTMLElement).closest<HTMLElement>("[data-tree-path]")
        if (!row) return
        const key = event.key.toLowerCase()
        if (!["c", "x", "v", "d"].includes(key)) return
        event.preventDefault()
        event.stopPropagation()
        runClipboardKey(workspacePath, key, row)
    }

    return (
        <ul ref={listRef} className="flex flex-col gap-[1px]" onKeyDown={onKeyDown}>
            {(rootNodes ?? []).map((node) => (
                <TreeNode key={node.path} node={node} root={workspacePath} depth={0} />
            ))}
        </ul>
    )
}

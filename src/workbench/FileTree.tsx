import { ChevronDown, ChevronRight, GitCompareArrows } from "lucide-react"
import { useEffect, useRef } from "react"
import { useTranslation } from "react-i18next"
import { logUserAction } from "@/features/logs/userAction"
import { FileIcon } from "../lib/fileIcons"
import type { FileNode } from "../lib/types"
import { contextMenuHandler } from "../state/contextMenuStore"
import { useFileTreeStore } from "../state/fileTreeStore"
import { changedPathSet, useGitStore } from "../state/gitStore"
import { useUiStore } from "../state/uiStore"
import { useWorkspaceStore } from "../state/workspaceStore"

// Repo-relative form of an absolute node path, matched against the git status
// (which reports paths relative to the repo root).
function relativePath(path: string, root: string | null) {
    if (root && path.startsWith(root + "/")) return path.slice(root.length + 1)
    return path
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
    const openDiffInGitMode = useUiStore((s) => s.openDiffInGitMode)
    const active = useWorkspaceStore(
        (s) => !node.isDir && s.groups[s.activeGroupIndex]?.activePath === node.path
    )
    // git status paths are relative to the repo root, which may sit above the
    // opened workspace (workspace = repo subdirectory). Use environment.root when
    // ready; fall back to workspacePath otherwise.
    const repoRoot = useGitStore((s) =>
        s.environment?.status === "ready" ? s.environment.root : workspacePath
    )
    const rel = relativePath(node.path, repoRoot)
    const isChanged = useGitStore((s) => !node.isDir && changedPathSet(s.status).has(rel))

    function onClick() {
        if (node.isDir) {
            void useFileTreeStore.getState().toggleDir(root, node.path)
        } else {
            openTab(node.path)
            void logUserAction("open_file", `open ${node.path}`)
        }
    }

    return (
        <li>
            <div className="group relative">
                <button
                    type="button"
                    onClick={onClick}
                    onContextMenu={workspacePath ? contextMenuHandler({
                        kind: "file",
                        workspacePath,
                        path: node.path,
                        isDirectory: node.isDir,
                        sourceGroupIndex
                    }) : undefined}
                    style={{ paddingLeft: `${14 + depth * 15}px` }}
                    className={
                        "flex h-[27px] w-full items-center gap-[7px] rounded-[8px] pr-[8px] text-left text-[12.5px] transition-colors duration-100 " +
                        (active
                            ? "bg-(--yz-active) text-(--ink-0) shadow-(--shadow-xs)"
                            : "hover:bg-(--yz-hover)")
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
                        onClick={() => openDiffInGitMode(rel)}
                        className="absolute top-1/2 right-[6px] flex size-[20px] -translate-y-1/2 items-center justify-center rounded-[6px] text-(--ink-3) opacity-0 transition-all duration-[130ms] group-hover:opacity-100 hover:bg-(--yz-hover) hover:text-(--yz-accent-ink)"
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

    // Persist/restore the nav scroller offset per workspace. The scroll
    // container is FileTree's parent (FilesNavContent's overflow-y-auto div).
    useEffect(() => {
        if (!workspacePath) return
        const scroller = listRef.current?.parentElement
        if (!scroller) return
        scroller.scrollTop = useFileTreeStore.getState().trees[workspacePath]?.scrollTop ?? 0
        const onScroll = () =>
            useFileTreeStore.getState().setScrollTop(workspacePath, scroller.scrollTop)
        scroller.addEventListener("scroll", onScroll, { passive: true })
        return () => scroller.removeEventListener("scroll", onScroll)
    }, [workspacePath])

    if (!workspacePath) return null

    return (
        <ul ref={listRef} className="flex flex-col gap-[1px]">
            {(rootNodes ?? []).map((node) => (
                <TreeNode key={node.path} node={node} root={workspacePath} depth={0} />
            ))}
        </ul>
    )
}

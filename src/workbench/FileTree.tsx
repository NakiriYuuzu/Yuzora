import { ChevronDown, ChevronRight, GitCompareArrows } from "lucide-react"
import { useEffect, useState } from "react"
import { useTranslation } from "react-i18next"
import { listDir } from "../lib/ipc"
import { logUserAction } from "@/features/logs/userAction"
import { FileIcon } from "../lib/fileIcons"
import type { FileNode } from "../lib/types"
import { contextMenuHandler } from "../state/contextMenuStore"
import { changedPathSet, useGitStore } from "../state/gitStore"
import { useUiStore } from "../state/uiStore"
import { useWorkspaceStore } from "../state/workspaceStore"

// Repo-relative form of an absolute node path, matched against the git status
// (which reports paths relative to the repo root).
function relativePath(path: string, root: string | null) {
    if (root && path.startsWith(root + "/")) return path.slice(root.length + 1)
    return path
}

function TreeNode({ node, depth }: { node: FileNode; depth: number }) {
    const { t } = useTranslation("menus")
    const [expanded, setExpanded] = useState(false)
    const [children, setChildren] = useState<FileNode[] | null>(null)
    const openTab = useWorkspaceStore((s) => s.openTab)
    const workspacePath = useWorkspaceStore((s) => s.workspacePath)
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

    async function onClick() {
        if (node.isDir) {
            if (!expanded && children === null) setChildren(await listDir(node.path))
            setExpanded(!expanded)
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
                    onContextMenu={contextMenuHandler("file", { path: node.path, isDir: node.isDir })}
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
                        <TreeNode key={child.path} node={child} depth={depth + 1} />
                    ))}
                </ul>
            )}
        </li>
    )
}

export function FileTree() {
    const workspacePath = useWorkspaceStore((s) => s.workspacePath)
    // A context-menu file op (new/rename/delete) bumps treeRevision: re-list the
    // roots and remount the subtree (keyed below) so cached children of expanded
    // folders are dropped and re-fetched, reflecting the change at any depth.
    const treeRevision = useWorkspaceStore((s) => s.treeRevision)
    const [roots, setRoots] = useState<FileNode[]>([])

    useEffect(() => {
        if (workspacePath) void listDir(workspacePath).then(setRoots)
    }, [workspacePath, treeRevision])

    if (!workspacePath) return null

    return (
        <ul key={treeRevision} className="flex flex-col gap-[1px]">
            {roots.map((node) => (
                <TreeNode key={node.path} node={node} depth={0} />
            ))}
        </ul>
    )
}

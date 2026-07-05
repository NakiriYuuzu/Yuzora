import { FileCode2 } from "lucide-react"

import { cn } from "@/lib/utils"
import { EmptyState } from "@/app/workbench/EmptyState"
import { useWorkspaceStore } from "../state/workspaceStore"
import { EditorPane } from "../editor/EditorPane"
import { documentGeneration } from "../editor/documentRegistry"
import { contextMenuHandler } from "../state/contextMenuStore"
import { TabBar } from "./TabBar"

const ACTION_BUTTON_CLASS =
    "flex size-[28px] items-center justify-center rounded-[9px] transition-all duration-150"
const ACTION_IDLE_CLASS = "text-(--ink-3) hover:bg-(--paper-3) hover:text-(--ink-1)"
const ACTION_ACTIVE_CLASS = "bg-(--yz-accent)/16 text-(--yz-accent-ink)"

export function EditorArea() {
    const groups = useWorkspaceStore((s) => s.groups)
    const splitRight = useWorkspaceStore((s) => s.splitRight)
    const closeSplit = useWorkspaceStore((s) => s.closeSplit)
    const setActiveGroup = useWorkspaceStore((s) => s.setActiveGroup)

    return (
        <div onContextMenu={contextMenuHandler("editor")} className="editor-groups flex min-h-0 min-w-0 flex-1">
            {groups.map((group, i) => {
                const last = i === groups.length - 1
                return (
                    <div
                        key={i}
                        onMouseDown={() => setActiveGroup(i)}
                        className={
                            "editor-group flex min-h-0 min-w-0 flex-1 flex-col" +
                            (i > 0 ? " border-l border-(--line-1)" : "")
                        }
                    >
                        <div className="group-header flex h-[44px] shrink-0 items-center gap-[3px] border-b border-(--line-1) bg-(--paper-0) px-[8px]">
                            <TabBar groupIndex={i} />
                            {last && (
                                <div className="group-actions flex shrink-0 items-center gap-[2px] pb-[7px]">
                                    <button
                                        type="button"
                                        aria-label={groups.length < 2 ? "向右分割" : "關閉分割"}
                                        title={groups.length < 2 ? "Split editor" : "Close split"}
                                        onClick={groups.length < 2 ? splitRight : closeSplit}
                                        className={cn(
                                            ACTION_BUTTON_CLASS,
                                            groups.length < 2 ? ACTION_IDLE_CLASS : ACTION_ACTIVE_CLASS
                                        )}
                                    >
                                        <svg
                                            width="15"
                                            height="15"
                                            viewBox="0 0 24 24"
                                            fill="none"
                                            stroke="currentColor"
                                            strokeWidth="1.8"
                                            strokeLinecap="round"
                                            aria-hidden="true"
                                        >
                                            <rect x="3" y="4" width="18" height="16" rx="2" />
                                            <path d="M12 4v16" />
                                        </svg>
                                    </button>
                                </div>
                            )}
                        </div>
                        {group.activePath ? (
                            <EditorPane
                                key={`${group.activePath}:${documentGeneration(group.activePath)}`}
                                path={group.activePath}
                            />
                        ) : (
                            <div className="empty-editor flex min-h-0 min-w-0 flex-1 items-center justify-center">
                                <EmptyState
                                    icon={FileCode2}
                                    title="Open a project to start editing"
                                    description="Files and tabs will appear here once a project is open."
                                />
                            </div>
                        )}
                    </div>
                )
            })}
        </div>
    )
}

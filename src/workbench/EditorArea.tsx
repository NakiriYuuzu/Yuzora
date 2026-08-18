import { FileCode2 } from "lucide-react"
import { useTranslation } from "react-i18next"

import { cn } from "@/lib/utils"
import { EmptyState } from "@/app/workbench/EmptyState"
import { HerdrTerminalPage } from "@/app/panels/HerdrTerminalPage"
import { PreviewPanel } from "@/app/panels/PreviewPanel"
import { isMarkdownPreviewTab, previewTabSourcePath } from "../lib/markdownPreviewTab"
import { PREVIEW_TAB_PATH, useWorkspaceStore } from "../state/workspaceStore"
import { EditorPane } from "../editor/EditorPane"
import { documentGeneration } from "../editor/documentRegistry"
import { TabBar } from "./TabBar"
import { MarkdownPreview } from "./MarkdownPreview"
import { ImageView, isImagePath } from "./ImageView"
import { SvgSplitView, isSvgPath } from "./SvgSplitView"

const ACTION_BUTTON_CLASS =
    "flex size-[28px] items-center justify-center rounded-[9px] transition-all duration-150"
const ACTION_IDLE_CLASS = "text-(--ink-3) hover:bg-(--paper-3) hover:text-(--ink-1)"
const ACTION_ACTIVE_CLASS = "bg-(--yz-accent)/16 text-(--yz-accent-ink)"

export function EditorArea() {
    const { t } = useTranslation("menus")
    const groups = useWorkspaceStore((s) => s.groups)
    const activeGroupIndex = useWorkspaceStore((s) => s.activeGroupIndex)
    const splitRight = useWorkspaceStore((s) => s.splitRight)
    const closeSplit = useWorkspaceStore((s) => s.closeSplit)
    const setActiveGroup = useWorkspaceStore((s) => s.setActiveGroup)

    return (
        <div className="editor-groups flex min-h-0 min-w-0 flex-1">
            {groups.map((group, i) => {
                const last = i === groups.length - 1
                const activeTab = group.tabs.find((tab) => tab.path === group.activePath)
                const herdrTabs = group.tabs.filter(
                    (tab) =>
                        tab.kind === "herdr-terminal" &&
                        tab.herdrSessionId &&
                        tab.terminalId
                )
                return (
                    <div
                        key={group.id ?? `legacy-group-${i}`}
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
                                        aria-label={
                                            groups.length < 2
                                                ? t("editorArea.splitRightAriaLabel")
                                                : t("editorArea.closeSplitAriaLabel")
                                        }
                                        title={
                                            groups.length < 2
                                                ? t("editorArea.splitTitle")
                                                : t("editorArea.closeSplitTitle")
                                        }
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
                        <div className="relative flex min-h-0 min-w-0 flex-1 overflow-hidden">
                            {herdrTabs.map((tab) => {
                                const tabVisible = tab.path === group.activePath
                                return (
                                    <div
                                        key={tab.path}
                                        className={cn(
                                            "absolute inset-0 min-h-0 min-w-0 transition-opacity duration-75 ease-out",
                                            tabVisible
                                                ? "opacity-100 pointer-events-auto"
                                                : "opacity-0 pointer-events-none"
                                        )}
                                        aria-hidden={!tabVisible}
                                        data-testid={`herdr-page-layer-${tab.path}`}
                                    >
                                        <HerdrTerminalPage
                                            herdrSessionId={tab.herdrSessionId!}
                                            runtimeTarget={tab.herdrRuntimeTarget}
                                            terminalId={tab.terminalId!}
                                            paneId={tab.paneId}
                                            herdrTabId={tab.herdrTabId}
                                            title={tab.name}
                                            pagePath={tab.path}
                                            active={tabVisible && i === activeGroupIndex}
                                            visible={tabVisible}
                                        />
                                    </div>
                                )
                            })}
                            {activeTab?.kind !== "herdr-terminal" && (
                                <div className="absolute inset-0 flex min-h-0 min-w-0">
                                    {(() => {
                                        if (
                                            group.activePath === PREVIEW_TAB_PATH ||
                                            activeTab?.kind === "preview"
                                        ) {
                                            return <PreviewPanel />
                                        }
                                        if (activeTab && isMarkdownPreviewTab(activeTab)) {
                                            const sourcePath = previewTabSourcePath(activeTab)
                                            if (!sourcePath) return null
                                            return (
                                                <MarkdownPreview
                                                    key={activeTab.path}
                                                    sourcePath={sourcePath}
                                                />
                                            )
                                        }
                                        if (!group.activePath) {
                                            return (
                                                <div className="empty-editor flex min-h-0 min-w-0 flex-1 items-center justify-center">
                                                    <EmptyState
                                                        icon={FileCode2}
                                                        title={t("editorArea.emptyTitle")}
                                                        description={t("editorArea.emptyDescription")}
                                                    />
                                                </div>
                                            )
                                        }
                                        if (isSvgPath(group.activePath)) {
                                            return (
                                                <SvgSplitView
                                                    key={`${group.activePath}:${documentGeneration(group.activePath)}`}
                                                    path={group.activePath}
                                                    groupIndex={i}
                                                />
                                            )
                                        }
                                        if (isImagePath(group.activePath)) {
                                            return (
                                                <ImageView
                                                    key={group.activePath}
                                                    path={group.activePath}
                                                />
                                            )
                                        }
                                        return (
                                            <EditorPane
                                                key={`${group.activePath}:${documentGeneration(group.activePath)}`}
                                                path={group.activePath}
                                                groupIndex={i}
                                            />
                                        )
                                    })()}
                                </div>
                            )}
                        </div>
                    </div>
                )
            })}
        </div>
    )
}

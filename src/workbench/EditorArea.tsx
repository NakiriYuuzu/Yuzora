import { useUiStore } from "../state/uiStore"
import { memo } from "react"
import { FileCode2, Globe, Search, SquareTerminal } from "lucide-react"
import { useTranslation } from "react-i18next"

import { cn } from "@/lib/utils"
import { Button } from "@/components/ui/button"
import { Kbd } from "@/components/ui/kbd"
import { bindingLabel, effectiveBinding, useKeyboardSettingsStore } from "@/state/keyboardSettingsStore"
import { openNewTerminalTab } from "@/terminal/openNewTerminalTab"
import { EmptyState } from "@/app/workbench/EmptyState"
import { HerdrTerminalPage } from "@/app/panels/HerdrTerminalPage"
import { PreviewPanel } from "@/app/panels/PreviewPanel"
import { isMarkdownPreviewTab, previewTabSourcePath } from "../lib/markdownPreviewTab"
import { PREVIEW_TAB_PATH, useWorkspaceStore } from "../state/workspaceStore"
import { RichMarkdownEditor } from "../editor/RichMarkdownEditor"
import { isMarkdownPath } from "./MarkdownPreview"
import { EditorPane } from "../editor/EditorPane"
import { documentGeneration } from "../editor/documentRegistry"
import { TabBar } from "./TabBar"
import { MarkdownPreview } from "./MarkdownPreview"
import { ImageView, isImagePath } from "./ImageView"
import { SvgSplitView, isSvgPath } from "./SvgSplitView"

// Keep background terminal trees mounted without rerendering every one when
// only the active tab changes. Runtime subscriptions still update each page.
const StableHerdrTerminalPage = memo(HerdrTerminalPage)

const ACTION_BUTTON_CLASS =
    "flex size-[28px] items-center justify-center rounded-[9px] transition-colors duration-150"
const ACTION_IDLE_CLASS = "text-(--ink-3) hover:bg-(--paper-3) hover:text-(--ink-1)"
const ACTION_ACTIVE_CLASS = "bg-(--yz-accent)/16 text-(--yz-accent-ink)"

export function EditorArea() {
    const { t } = useTranslation("menus")
    const editorSurfaceVisible = useUiStore((s) => s.mode === "files" || s.mode === "ade")
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
                        <div data-tauri-drag-region="deep" className="group-header flex h-[44px] shrink-0 items-center gap-[3px] border-b border-(--line-1) bg-(--paper-0) px-[8px]">
                            <TabBar groupIndex={i} />
                            {last && (
                                <div className="group-actions flex shrink-0 items-center gap-[2px]">
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
                                const tabVisible = editorSurfaceVisible && tab.path === group.activePath
                                return (
                                    <div
                                        key={tab.path}
                                        className={cn(
                                            "absolute inset-0 min-h-0 min-w-0",
                                            tabVisible
                                                ? "visible pointer-events-auto"
                                                : "invisible pointer-events-none"
                                        )}
                                        aria-hidden={!tabVisible}
                                        inert={!tabVisible}
                                        data-testid={`herdr-page-layer-${tab.path}`}
                                    >
                                        <StableHerdrTerminalPage
                                            herdrSessionId={tab.herdrSessionId!}
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
                                                    <EmptyEditorState groupIndex={i} />
                                                </div>
                                            )
                                        }
                                        if (isMarkdownPath(group.activePath)) {
                                            return <RichMarkdownEditor key={`${group.activePath}:${documentGeneration(group.activePath)}`} path={group.activePath} groupIndex={i} />
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

// The app root font is 13px, so rem-based shadcn sizes render too small for a primary empty-state action.
const EMPTY_ACTION_CLASS = "h-[30px] px-[11px] text-[12.5px]"

function ShortcutHint({ binding }: { binding: string }) {
    return (
        <Kbd className="ml-[2px] h-auto rounded-[4px] bg-(--yz-active) px-[5px] py-px font-mono text-[10.5px] text-(--ink-3)">
            {bindingLabel(binding)}
        </Kbd>
    )
}

/**
 * Empty group: explain the state (no project vs. no open tab) and offer the
 * common next steps instead of a dead end.
 */
function EmptyEditorState({ groupIndex }: { groupIndex: number }) {
    const { t } = useTranslation("menus")
    const hasWorkspace = useWorkspaceStore((s) => Boolean(s.workspacePath))
    const paletteBinding = useKeyboardSettingsStore((s) => effectiveBinding("commandPalette", s.overrides))
    const terminalBinding = useKeyboardSettingsStore((s) => effectiveBinding("newTerminal", s.overrides))
    const browserBinding = useKeyboardSettingsStore((s) => effectiveBinding("toggleBrowser", s.overrides))
    const openPreviewTab = useWorkspaceStore((s) => s.openPreviewTab)
    return (
        <EmptyState
            icon={FileCode2}
            title={t(hasWorkspace ? "editorArea.noTabsTitle" : "editorArea.emptyTitle")}
            description={t(hasWorkspace ? "editorArea.noTabsDescription" : "editorArea.emptyDescription")}
            actions={
                <>
                    <Button variant="outline" className={EMPTY_ACTION_CLASS} onClick={() => useUiStore.getState().requestOpenPalette()}>
                        <Search data-icon="inline-start" aria-hidden="true" />
                        {t("editorArea.actionSearch")}
                        <ShortcutHint binding={paletteBinding} />
                    </Button>
                    <Button variant="outline" className={EMPTY_ACTION_CLASS} onClick={() => void openNewTerminalTab(groupIndex)}>
                        <SquareTerminal data-icon="inline-start" aria-hidden="true" />
                        {t("editorArea.actionNewTerminal")}
                        <ShortcutHint binding={terminalBinding} />
                    </Button>
                    <Button variant="outline" className={EMPTY_ACTION_CLASS} onClick={() => openPreviewTab(groupIndex)}>
                        <Globe data-icon="inline-start" aria-hidden="true" />
                        {t("editorArea.actionBrowser")}
                        <ShortcutHint binding={browserBinding} />
                    </Button>
                </>
            }
        />
    )
}

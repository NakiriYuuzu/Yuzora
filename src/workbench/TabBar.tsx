import { Globe } from "lucide-react"
import { useTranslation } from "react-i18next"
import { type TabInfo, useWorkspaceStore } from "../state/workspaceStore"
import { useUiStore } from "../state/uiStore"
import { useConfirmDialogStore } from "../state/confirmDialogStore"
import { dropDocument } from "../editor/documentRegistry"
import { saveDirtyTab } from "../editor/saveDocument"
import { logUserAction } from "@/features/logs/userAction"
import { FileIcon } from "../lib/fileIcons"
import { workspacePathForDisplay } from "../lib/paths"
import { contextMenuHandler } from "../state/contextMenuStore"
import { isMarkdownPath, useMarkdownPreviewStore } from "./MarkdownPreview"
import { isSvgPath, useSvgPreviewStore } from "./SvgSplitView"

export function TabBar({ groupIndex }: { groupIndex: number }) {
    const { t } = useTranslation("menus")
    const group = useWorkspaceStore((s) => s.groups[groupIndex])
    const setActiveTab = useWorkspaceStore((s) => s.setActiveTab)
    const closeTab = useWorkspaceStore((s) => s.closeTab)
    const closePreviewTab = useWorkspaceStore((s) => s.closePreviewTab)
    const workspacePath = useWorkspaceStore((s) => s.workspacePath)
    const previewOpen = useMarkdownPreviewStore((s) => s.openPaths)
    const togglePreview = useMarkdownPreviewStore((s) => s.toggle)
    const closePreview = useMarkdownPreviewStore((s) => s.close)
    const svgClosedPaths = useSvgPreviewStore((s) => s.closedPaths)
    const toggleSvgPreview = useSvgPreviewStore((s) => s.toggle)
    const forgetSvgPreview = useSvgPreviewStore((s) => s.forget)
    if (!group) return null

    async function onClose(tab: TabInfo) {
        // The preview tab holds no document — close it without a dirty prompt and
        // without touching document/markdown-preview registries (its path is a
        // sentinel, not a real file).
        if (tab.kind === "preview") {
            closePreviewTab()
            void logUserAction("close_tab", `close ${tab.name}`)
            return
        }
        if (tab.dirty) {
            const decision = await useConfirmDialogStore.getState().requestUnsavedDecision({
                title: t("unsavedDialog.closeTabTitle"),
                description: t("unsavedDialog.closeTabDescription", { name: tab.name }),
                saveLabel: t("unsavedDialog.save")
            })
            if (decision === "cancel") return
            if (decision === "save") {
                const outcome = await saveDirtyTab(tab.path)
                if (outcome.kind !== "saved") return
            }
        }
        closeTab(groupIndex, tab.path)
        dropDocument(tab.path)
        closePreview(tab.path)
        // Reopening an SVG returns to the default-open preview state.
        forgetSvgPreview(tab.path)
        void logUserAction("close_tab", `close ${tab.path}`)
    }

    return (
        <div className="yzs flex h-[44px] min-w-0 flex-1 items-center gap-[3px] overflow-x-auto overflow-y-hidden">
            {group.tabs.length === 0 && (
                <span className="px-[10px] text-[12px] text-(--ink-4)">{t("tabBar.noOpenTabs")}</span>
            )}
            {group.tabs.map((tab) => {
                const active = tab.path === group.activePath
                return (
                    <span
                        key={tab.path}
                        onContextMenu={contextMenuHandler({
                            kind: "tab",
                            workspacePath,
                            path: tab.path,
                            groupIndex
                        })}
                        className={
                            "tab flex h-[30px] shrink-0 items-center gap-[8px] rounded-[9px] pr-[8px] pl-[12px] transition-all duration-150 ease-(--ease-out) " +
                            (active
                                ? "active bg-(--yz-active) text-(--ink-0) shadow-(--shadow-xs)"
                                : "text-(--ink-3) hover:bg-(--yz-hover)")
                        }
                    >
                        {tab.kind === "preview" ? (
                            <Globe className="size-[15px] shrink-0" aria-hidden="true" />
                        ) : (
                            <FileIcon fileName={tab.name} className="size-[15px] shrink-0" />
                        )}
                        <button
                            type="button"
                            title={workspacePathForDisplay(tab.path)}
                            className={
                                "tab-name max-w-[140px] truncate text-left text-[12.5px] whitespace-nowrap " +
                                (active ? "font-semibold" : "font-medium")
                            }
                            onClick={() => {
                                setActiveTab(groupIndex, tab.path)
                                void logUserAction("switch_tab", `switch to ${tab.path}`)
                            }}
                        >
                            {tab.name}
                        </button>
                        {tab.externallyModified && (
                            <span
                                role="button"
                                tabIndex={0}
                                aria-label={t("tabBar.resolveExternalChanges", { name: tab.name })}
                                className="ext-dot shrink-0 cursor-pointer text-[12px] font-semibold text-[#c8521f]"
                                title={t("tabBar.externallyModifiedTitle")}
                                onClick={(e) => {
                                    e.stopPropagation()
                                    useUiStore.getState().openResolver(tab.path)
                                }}
                            >
                                ↻
                            </span>
                        )}
                        {tab.dirty && (
                            <span
                                className="dirty-dot size-[7px] shrink-0 rounded-full bg-[#d68a0c]"
                                title={t("tabBar.unsavedChangesTitle")}
                                aria-hidden="true"
                            />
                        )}
                        {(isMarkdownPath(tab.name) || isSvgPath(tab.name)) && (
                            <button
                                type="button"
                                className={
                                    "preview-toggle flex size-[18px] shrink-0 items-center justify-center rounded-[6px] transition-colors " +
                                    ((isMarkdownPath(tab.name)
                                        ? previewOpen[tab.path]
                                        : !svgClosedPaths[tab.path])
                                        ? "bg-(--yz-accent)/16 text-(--yz-accent-ink)"
                                        : "text-(--ink-3) hover:bg-(--paper-3) hover:text-(--ink-0)")
                                }
                                aria-label={t("tabBar.togglePreview", { name: tab.name })}
                                aria-pressed={
                                    isMarkdownPath(tab.name)
                                        ? !!previewOpen[tab.path]
                                        : !svgClosedPaths[tab.path]
                                }
                                title={
                                    isMarkdownPath(tab.name)
                                        ? t("tabBar.toggleMarkdownPreviewTitle")
                                        : t("tabBar.toggleSvgPreviewTitle")
                                }
                                onClick={(e) => {
                                    e.stopPropagation()
                                    // panel gate 綁 active tab；先 activate 被點的 tab 再 toggle，
                                    // 使 aria-pressed 與可見 panel 一致（非 active tab 不再說謊）（R11-2）。
                                    setActiveTab(groupIndex, tab.path)
                                    if (isMarkdownPath(tab.name)) {
                                        togglePreview(tab.path)
                                        void logUserAction("toggle_md_preview", `toggle preview ${tab.path}`)
                                    } else {
                                        toggleSvgPreview(tab.path)
                                        void logUserAction("toggle_svg_preview", `toggle preview ${tab.path}`)
                                    }
                                }}
                            >
                                <svg
                                    width="12"
                                    height="12"
                                    viewBox="0 0 24 24"
                                    fill="none"
                                    stroke="currentColor"
                                    strokeWidth="1.9"
                                    strokeLinecap="round"
                                    strokeLinejoin="round"
                                    aria-hidden="true"
                                >
                                    <path d="M2 12s3.5-7 10-7 10 7 10 7-3.5 7-10 7-10-7-10-7Z" />
                                    <circle cx="12" cy="12" r="2.6" />
                                </svg>
                            </button>
                        )}
                        <button
                            type="button"
                            className="tab-close flex size-[18px] shrink-0 items-center justify-center rounded-[6px] text-(--ink-3) transition-colors hover:bg-(--paper-3) hover:text-(--ink-0)"
                            aria-label={t("tabBar.close", { name: tab.name })}
                            onClick={() => void onClose(tab)}
                        >
                            <svg
                                width="11"
                                height="11"
                                viewBox="0 0 24 24"
                                fill="none"
                                stroke="currentColor"
                                strokeWidth="2.2"
                                strokeLinecap="round"
                                aria-hidden="true"
                            >
                                <path d="M18 6 6 18M6 6l12 12" />
                            </svg>
                        </button>
                    </span>
                )
            })}
        </div>
    )
}

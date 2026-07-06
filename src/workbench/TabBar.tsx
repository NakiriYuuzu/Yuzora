import { confirm } from "@tauri-apps/plugin-dialog"
import { useWorkspaceStore } from "../state/workspaceStore"
import { useUiStore } from "../state/uiStore"
import { dropDocument } from "../editor/documentRegistry"
import { logUserAction } from "@/features/logs/userAction"
import { contextMenuHandler } from "../state/contextMenuStore"
import { MarkdownPreview, isMarkdownPath, useMarkdownPreviewStore } from "./MarkdownPreview"

// Design reference tab strip: extension chip colors (md blue, code orange).
function extChip(name: string) {
    const ext = name.includes(".") ? name.split(".").pop()!.toLowerCase() : ""
    return {
        label: ext.slice(0, 2) || "··",
        color: ext === "md" ? "#2456cc" : "#c8521f"
    }
}

export function TabBar({ groupIndex }: { groupIndex: number }) {
    const group = useWorkspaceStore((s) => s.groups[groupIndex])
    const setActiveTab = useWorkspaceStore((s) => s.setActiveTab)
    const closeTab = useWorkspaceStore((s) => s.closeTab)
    const activeGroupIndex = useWorkspaceStore((s) => s.activeGroupIndex)
    const mode = useUiStore((s) => s.mode)
    const previewOpen = useMarkdownPreviewStore((s) => s.openPaths)
    const togglePreview = useMarkdownPreviewStore((s) => s.toggle)
    const closePreview = useMarkdownPreviewStore((s) => s.close)
    if (!group) return null

    async function onClose(path: string, dirty: boolean) {
        if (dirty) {
            const ok = await confirm("檔案有未儲存的變更，確定關閉？")
            if (!ok) return
        }
        closeTab(groupIndex, path)
        dropDocument(path)
        closePreview(path)
        void logUserAction("close_tab", `close ${path}`)
    }

    return (
        <div className="yzs flex h-[44px] min-w-0 flex-1 items-center gap-[3px] overflow-x-auto overflow-y-hidden">
            {group.tabs.length === 0 && (
                <span className="px-[10px] text-[12px] text-(--ink-4)">No open tabs</span>
            )}
            {group.tabs.map((tab) => {
                const active = tab.path === group.activePath
                const chip = extChip(tab.name)
                return (
                    <span
                        key={tab.path}
                        onContextMenu={contextMenuHandler("tab", { path: tab.path, groupIndex })}
                        className={
                            "tab flex h-[30px] shrink-0 items-center gap-[8px] rounded-[9px] pr-[8px] pl-[12px] transition-all duration-150 ease-(--ease-out) " +
                            (active
                                ? "active bg-(--yz-active) text-(--ink-0) shadow-(--shadow-xs)"
                                : "text-(--ink-3) hover:bg-(--yz-hover)")
                        }
                    >
                        <span
                            aria-hidden="true"
                            className="flex size-[15px] shrink-0 items-center justify-center rounded-[4px] font-mono text-[8px] font-bold text-white"
                            style={{ background: chip.color }}
                        >
                            {chip.label}
                        </span>
                        <button
                            type="button"
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
                                aria-label={`Resolve external changes ${tab.name}`}
                                className="ext-dot shrink-0 cursor-pointer text-[12px] font-semibold text-[#c8521f]"
                                title="外部已變更（點擊開啟解決器）"
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
                                title="未儲存的變更"
                                aria-hidden="true"
                            />
                        )}
                        {isMarkdownPath(tab.name) && (
                            <button
                                type="button"
                                className={
                                    "preview-toggle flex size-[18px] shrink-0 items-center justify-center rounded-[6px] transition-colors " +
                                    (previewOpen[tab.path]
                                        ? "bg-(--yz-accent)/16 text-(--yz-accent-ink)"
                                        : "text-(--ink-3) hover:bg-(--paper-3) hover:text-(--ink-0)")
                                }
                                aria-label={`Toggle preview ${tab.name}`}
                                aria-pressed={!!previewOpen[tab.path]}
                                title="切換 Markdown 預覽"
                                onClick={(e) => {
                                    e.stopPropagation()
                                    // panel gate 綁 active tab；先 activate 被點的 tab 再 toggle，
                                    // 使 aria-pressed 與可見 panel 一致（非 active tab 不再說謊）（R11-2）。
                                    setActiveTab(groupIndex, tab.path)
                                    togglePreview(tab.path)
                                    void logUserAction("toggle_md_preview", `toggle preview ${tab.path}`)
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
                            aria-label={`Close ${tab.name}`}
                            onClick={() => void onClose(tab.path, tab.dirty)}
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
            {mode === "files" &&
                groupIndex === activeGroupIndex &&
                group.activePath &&
                isMarkdownPath(group.activePath.split("/").pop() ?? "") &&
                previewOpen[group.activePath] && (
                    <MarkdownPreview key={group.activePath} path={group.activePath} />
                )}
        </div>
    )
}

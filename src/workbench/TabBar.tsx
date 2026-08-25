import { useRef, type DragEvent, type KeyboardEvent } from "react"
import { Bot, Globe, Plus, SquareTerminal } from "lucide-react"
import { useTranslation } from "react-i18next"
import {
    isFileTab,
    isMarkdownPreviewTab,
    previewTabSourcePath
} from "../lib/markdownPreviewTab"
import { type TabInfo, useWorkspaceStore } from "../state/workspaceStore"
import type { HerdrTabInfo } from "../lib/herdrTypes"
import { useUiStore } from "../state/uiStore"
import { useConfirmDialogStore } from "../state/confirmDialogStore"
import { useHerdrStore } from "../state/herdrStore"
import { dropDocument } from "../editor/documentRegistry"
import { saveDirtyTab } from "../editor/saveDocument"
import { logUserAction } from "@/features/logs/userAction"
import { FileIcon } from "../lib/fileIcons"
import { workspacePathForDisplay } from "../lib/paths"
import { contextMenuHandler } from "../state/contextMenuStore"
import { isMarkdownPath } from "./MarkdownPreview"
import { isSvgPath, useSvgPreviewStore } from "./SvgSplitView"
import {
    DropdownMenu,
    DropdownMenuContent,
    DropdownMenuGroup,
    DropdownMenuItem,
    DropdownMenuLabel,
    DropdownMenuTrigger
} from "@/components/ui/dropdown-menu"
import { ScrollArea } from "@/components/ui/scroll-area"
import {
    closeHerdrTabIdempotently,
    openCreatedHerdrTabAndRequestName
} from "@/lib/herdrTabActions"
import { herdrTabMove } from "@/lib/herdrIpc"
import { herdrInsertIndexForProjectedDrop } from "@/lib/workbenchTabReorder"
import { showActionError } from "@/lib/actionFeedback"
import { requestAppConfirmation } from "@/state/appDialogStore"

export function TabBar({ groupIndex }: { groupIndex: number }) {
    const { t } = useTranslation("menus")
    const activationIntentRef = useRef(0)
    const closingHerdrPagesRef = useRef(new Set<string>())
    const group = useWorkspaceStore((s) => s.groups[groupIndex])
    const setActiveTab = useWorkspaceStore((s) => s.setActiveTab)
    const closeTab = useWorkspaceStore((s) => s.closeTab)
    const reorderProjectedTab = useWorkspaceStore((s) => s.reorderProjectedTab)
    const draggedTabPathRef = useRef<string | null>(null)
    const closePreviewTab = useWorkspaceStore((s) => s.closePreviewTab)
    const closeMarkdownPreviewTab = useWorkspaceStore((s) => s.closeMarkdownPreviewTab)
    const toggleMarkdownPreview = useWorkspaceStore((s) => s.toggleMarkdownPreview)
    const markdownPreviewSourceKey = useWorkspaceStore((s) => {
        const sources: string[] = []
        for (const candidate of s.groups) {
            for (const tab of candidate.tabs) {
                if (!isMarkdownPreviewTab(tab)) continue
                const source = previewTabSourcePath(tab)
                if (source) sources.push(source)
            }
        }
        return sources.sort().join("\0")
    })
    const markdownPreviewSources = new Set(
        markdownPreviewSourceKey ? markdownPreviewSourceKey.split("\0") : []
    )
    const workspacePath = useWorkspaceStore((s) => s.workspacePath)
    const herdrSessions = useHerdrStore((s) => s.sessions)
    const herdrRuntimes = useHerdrStore((s) => s.runtimesBySession)
    const herdrSelectedSessionName = useHerdrStore((s) => s.selectedSessionName)
    const herdrSelectedSpaceId = useHerdrStore((s) => s.selectedSpaceId)
    const herdrSelectedSnapshot = useHerdrStore((s) => s.snapshot)
    const canCreateHerdrTerminal = useHerdrStore((s) => s.canCreateTerminal())
    const canFocusHerdrTab = useHerdrStore((s) => s.canFocusSelectedTab())
    const canMoveHerdrTab = useHerdrStore((s) => s.canMoveSelectedTab())
    const createHerdrTerminal = useHerdrStore((s) => s.createTerminalInSelectedSpace)
    const activateHerdrTab = useHerdrStore((s) => s.activateTab)
    const mode = useUiStore((s) => s.mode)
    const setMode = useUiStore((s) => s.setMode)
    const svgClosedPaths = useSvgPreviewStore((s) => s.closedPaths)
    const toggleSvgPreview = useSvgPreviewStore((s) => s.toggle)
    const forgetSvgPreview = useSvgPreviewStore((s) => s.forget)
    if (!group) return null

    const visibleHerdrTabs =
        herdrSelectedSnapshot?.tabs.filter((tab) => tab.workspaceId === herdrSelectedSpaceId) ?? []
    const runtimeWorkspaceByTabId = new Map(
        herdrSelectedSnapshot?.tabs.map((tab) => [tab.id, tab.workspaceId]) ?? []
    )
    const projectedTabs = group.tabs.filter((tab) => {
        if (tab.kind !== "herdr-terminal") return true
        // Before Herdr selection is hydrated, retain the last-known page strip.
        // Once a selected Space exists, projection becomes strictly Space-owned.
        if (!herdrSelectedSessionName || !herdrSelectedSpaceId) return true
        const sessionName =
            tab.herdrSessionId === "live"
                ? (herdrSessions.find((session) => session.default) ?? herdrSessions[0])?.name ?? "live"
                : tab.herdrSessionId ?? "live"
        if (sessionName !== herdrSelectedSessionName) return false
        const workspaceId =
            tab.herdrWorkspaceId ??
            (tab.herdrTabId ? runtimeWorkspaceByTabId.get(tab.herdrTabId) : null)
        return workspaceId === herdrSelectedSpaceId
    })
    const showHerdrTabsMenu = mode === "ade" && Boolean(herdrSelectedSessionName)

    function onOpenBrowserTab() {
        useWorkspaceStore.getState().openPreviewTab(groupIndex)
        void logUserAction("open_preview_tab", "open browser tab")
    }

    async function onCreateHerdrTab() {
        const created = await createHerdrTerminal()
        if (!created) return
        setMode("ade")
        try {
            await openCreatedHerdrTabAndRequestName({
                sessionName: created.herdrSessionId,
                workspaceId: created.workspaceId,
                terminalId: created.terminalId,
                title: created.title,
                paneId: created.paneId,
                tabId: created.tabId,
                groupIndex
            })
        } catch (error) {
            await showActionError(t("contextMenu.cmHerdrRenameTab"), error)
        }
    }

    async function onOpenHerdrTab(tabId: string) {
        const tab = visibleHerdrTabs.find((candidate) => candidate.id === tabId)
        if (!tab) return
        activationIntentRef.current += 1
        await activateHerdrTab(tab)
    }

    async function onActivateOpenHerdrPage(tab: TabInfo, runtimeTab: HerdrTabInfo) {
        const activationIntent = ++activationIntentRef.current
        const previousPath =
            useWorkspaceStore.getState().groups[groupIndex]?.activePath ?? null
        // Keep already-open terminal pages responsive while the Herdr focus
        // transaction runs. The store action dedupes the page after success.
        setActiveTab(groupIndex, tab.path)
        const result = await activateHerdrTab(runtimeTab)
        if (activationIntentRef.current !== activationIntent) return
        if (result?.ok !== false) return

        // Roll back only if this failed activation still owns the visible page;
        // a newer click must never be overwritten by an older completion.
        const currentPath =
            useWorkspaceStore.getState().groups[groupIndex]?.activePath ?? null
        if (
            currentPath === tab.path &&
            previousPath &&
            previousPath !== tab.path
        ) {
            setActiveTab(groupIndex, previousPath)
        }
    }

    function onActivate(tab: TabInfo, herdrRuntimeTab: HerdrTabInfo | undefined) {
        if (tab.kind === "herdr-terminal" && herdrRuntimeTab) {
            void onActivateOpenHerdrPage(tab, herdrRuntimeTab)
        } else {
            setActiveTab(groupIndex, tab.path)
        }
        void logUserAction("switch_tab", `switch to ${tab.path}`)
    }

    function resolvedHerdrWorkspaceId(tab: TabInfo) {
        return tab.herdrWorkspaceId ??
            (tab.herdrTabId ? runtimeWorkspaceByTabId.get(tab.herdrTabId) : null)
    }

    function canReorderHerdrTab(tab: TabInfo) {
        if (!canMoveHerdrTab || !(tab.herdrTabId ?? "").trim()) return false
        if (!herdrSelectedSpaceId) return true
        return resolvedHerdrWorkspaceId(tab) === herdrSelectedSpaceId
    }

    function canDragTab(tab: TabInfo) {
        return tab.kind !== "herdr-terminal" || canReorderHerdrTab(tab)
    }

    function onTabDragStart(event: DragEvent<HTMLButtonElement>, tab: TabInfo) {
        if (!canDragTab(tab)) {
            event.preventDefault()
            return
        }
        draggedTabPathRef.current = tab.path
        if (event.dataTransfer) {
            event.dataTransfer.effectAllowed = "move"
            event.dataTransfer.setData("text/plain", tab.path)
        }
    }

    function onTabDragOver(event: DragEvent<HTMLSpanElement>) {
        if (!draggedTabPathRef.current && event.dataTransfer.types.includes("text/plain")) {
            event.preventDefault()
            return
        }
        if (draggedTabPathRef.current) event.preventDefault()
    }

    async function moveHerdrTab(sessionName: string, tabId: string, insertIndex: number) {
        try {
            await herdrTabMove({ sessionName, tabId, insertIndex })
            useHerdrStore.getState().bumpTopologyRevision()
            await useHerdrStore.getState().refreshSnapshot(sessionName)
            void logUserAction("reorder_tab", `move herdr ${sessionName}:${tabId}`)
        } catch (error) {
            await showActionError(t("tabBar.reorderHerdrFailed"), error)
        }
    }

    function prepareTabReorder(
        sourceTab: TabInfo,
        destProjectedIndex: number
    ): (() => void) | null {
        const sourceProjectedIndex = projectedTabs.findIndex(
            (candidate) => candidate.path === sourceTab.path
        )
        if (
            sourceProjectedIndex < 0 ||
            destProjectedIndex < 0 ||
            destProjectedIndex >= projectedTabs.length ||
            sourceProjectedIndex === destProjectedIndex
        ) {
            return null
        }
        if (sourceTab.kind === "herdr-terminal") {
            if (!canReorderHerdrTab(sourceTab)) return null
            const insertIndex = herdrInsertIndexForProjectedDrop(
                projectedTabs,
                sourceTab.path,
                destProjectedIndex,
                herdrSelectedSpaceId,
                runtimeWorkspaceByTabId
            )
            const tabId = sourceTab.herdrTabId?.trim()
            if (insertIndex === null || !tabId) return null
            const sessionName =
                sourceTab.herdrSessionId === "live"
                    ? (herdrSessions.find((session) => session.default) ?? herdrSessions[0])?.name ??
                      "live"
                    : sourceTab.herdrSessionId ?? herdrSelectedSessionName ?? "live"
            return () => void moveHerdrTab(sessionName, tabId, insertIndex)
        }
        return () => {
            reorderProjectedTab(
                groupIndex,
                sourceTab.path,
                destProjectedIndex,
                projectedTabs
            )
            void logUserAction("reorder_tab", `reorder ${sourceTab.path}`)
        }
    }

    function onTabDrop(event: DragEvent<HTMLSpanElement>, destProjectedIndex: number) {
        event.preventDefault()
        const sourcePath =
            draggedTabPathRef.current || event.dataTransfer?.getData("text/plain")
        draggedTabPathRef.current = null
        if (!sourcePath) return
        const sourceTab = projectedTabs.find((tab) => tab.path === sourcePath)
        if (!sourceTab) return
        prepareTabReorder(sourceTab, destProjectedIndex)?.()
    }

    function onTabKeyDown(
        event: KeyboardEvent<HTMLButtonElement>,
        tab: TabInfo,
        projectedIndex: number
    ) {
        if (
            !event.altKey ||
            event.ctrlKey ||
            event.metaKey ||
            event.shiftKey ||
            (event.key !== "ArrowLeft" && event.key !== "ArrowRight")
        ) {
            return
        }
        const destination = projectedIndex + (event.key === "ArrowLeft" ? -1 : 1)
        const reorder = prepareTabReorder(tab, destination)
        if (!reorder) return
        event.preventDefault()
        event.stopPropagation()
        reorder()
    }

    async function onClose(tab: TabInfo) {
        // Browser singleton preview holds no document — close it without a dirty
        // prompt or dropDocument. Markdown preview is a store-owned adjacent
        // EditorGroup tab and is closed through closeMarkdownPreviewTab below.
        if (tab.kind === "preview") {
            closePreviewTab()
            void logUserAction("close_tab", `close ${tab.name}`)
            return
        }
        if (isMarkdownPreviewTab(tab)) {
            closeMarkdownPreviewTab(groupIndex, tab.path)
            void logUserAction("close_tab", `close ${tab.name}`)
            return
        }
        // The top-level close button mirrors Herdr's destructive tab close.
        // Runtime success is the commit point: connector release and local page
        // removal happen only afterwards, so a rejected/failed close keeps the
        // user's terminal surface intact. App teardown remains release-only.
        if (tab.kind === "herdr-terminal") {
            if (closingHerdrPagesRef.current.has(tab.path)) return
            closingHerdrPagesRef.current.add(tab.path)
            const sessionName =
                tab.herdrSessionId === "live"
                    ? (herdrSessions.find((session) => session.default) ?? herdrSessions[0])?.name ?? "live"
                    : tab.herdrSessionId ?? "live"
            const targetSnapshot =
                herdrRuntimes[sessionName]?.snapshot ??
                (herdrSelectedSessionName === sessionName ? herdrSelectedSnapshot : null)
            const target =
                targetSnapshot?.terminals.find(
                    (terminal) => terminal.terminalId === tab.terminalId
                ) ??
                targetSnapshot?.agents.find(
                    (agent) => agent.terminalId === tab.terminalId
                )
            const tabId = tab.herdrTabId ?? target?.tabId ?? null
            try {
                if (!tabId) throw new Error("Herdr tab ID unavailable")
                const accepted = await requestAppConfirmation({
                    title: t("contextMenu.confirm.closeHerdrTabTitle"),
                    description: t("contextMenu.confirm.closeHerdrTab", { name: tab.name }),
                    kind: "warning",
                    destructive: true
                })
                if (!accepted) return
                await closeHerdrTabIdempotently(sessionName, tabId)
                await useHerdrStore.getState().releaseAttachmentsForPage(tab.path)
                useWorkspaceStore.getState().closeTabsByPath([tab.path])
                useHerdrStore.getState().bumpTopologyRevision()
                void useHerdrStore.getState().refreshSnapshot(sessionName).catch(() => undefined)
                void logUserAction("close_tab", `close herdr ${sessionName}:${tabId}`)
            } catch (error) {
                await showActionError(t("contextMenu.cmHerdrCloseTab"), error)
            } finally {
                closingHerdrPagesRef.current.delete(tab.path)
            }
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
        if (isFileTab(tab)) dropDocument(tab.path)
        // Reopening an SVG returns to the default-open preview state.
        forgetSvgPreview(tab.path)
        void logUserAction("close_tab", `close ${tab.path}`)
    }

    return (
        <ScrollArea
            className="h-[44px] min-w-0 flex-1"
            orientation="horizontal"
            contentClassName="flex h-[44px] w-max items-center gap-[3px]"
        >
            {projectedTabs.length === 0 && (
                <span className="px-[10px] text-[12px] text-(--ink-4)">{t("tabBar.noOpenTabs")}</span>
            )}
            {projectedTabs.map((tab, index) => {
                const active = tab.path === group.activePath
                const herdrSessionName =
                    tab.herdrSessionId === "live"
                        ? (herdrSessions.find((session) => session.default) ?? herdrSessions[0])?.name ?? "live"
                        : tab.herdrSessionId ?? "live"
                const herdrSnapshot =
                    herdrRuntimes[herdrSessionName]?.snapshot ??
                    (herdrSelectedSessionName === herdrSessionName ? herdrSelectedSnapshot : null)
                const herdrTarget =
                    herdrSnapshot?.terminals.find((terminal) =>
                        tab.herdrTabId
                            ? terminal.tabId === tab.herdrTabId
                            : terminal.terminalId === tab.terminalId
                    ) ??
                    herdrSnapshot?.agents.find((agent) =>
                        tab.herdrTabId
                            ? agent.tabId === tab.herdrTabId
                            : agent.terminalId === tab.terminalId
                    )
                const herdrRuntimeTab = herdrSnapshot?.tabs.find(
                    (runtimeTab) =>
                        runtimeTab.id === (tab.herdrTabId ?? herdrTarget?.tabId)
                )
                const tabContextMenu =
                    tab.kind === "herdr-terminal"
                        ? contextMenuHandler({
                              kind: "herdrTab",
                              sessionName: herdrSessionName,
                              tabId: tab.herdrTabId ?? herdrTarget?.tabId ?? "",
                              workspaceId:
                                  herdrRuntimeTab?.workspaceId ?? herdrTarget?.workspaceId ?? null,
                              label: tab.name,
                              pagePath: tab.path
                          })
                        : contextMenuHandler({
                              kind: "tab",
                              workspacePath,
                              path: tab.path,
                              groupIndex
                          })
                return (
                    <span
                        key={tab.path}
                        onContextMenu={tabContextMenu}
                        onDragOver={onTabDragOver}
                        onDrop={(event) => void onTabDrop(event, index)}
                        className={
                            "tab flex h-[30px] shrink-0 items-center gap-[8px] rounded-[9px] pr-[8px] pl-[12px] transition-all duration-150 ease-(--ease-out) " +
                            (active
                                ? "active bg-(--yz-active) text-(--ink-0) shadow-(--shadow-xs)"
                                : "text-(--ink-3) hover:bg-(--yz-hover)")
                        }
                    >
                        <button
                            type="button"
                            draggable={canDragTab(tab)}
                            title={
                                tab.kind === "herdr-terminal"
                                    ? tab.name
                                    : workspacePathForDisplay(tab.path)
                            }
                            className={
                                "tab-name flex min-w-0 items-center gap-[8px] text-left text-[12.5px] whitespace-nowrap " +
                                (active ? "font-semibold" : "font-medium")
                            }
                            aria-keyshortcuts="Alt+ArrowLeft Alt+ArrowRight"
                            onDragStart={(event) => onTabDragStart(event, tab)}
                            onDragEnd={() => {
                                draggedTabPathRef.current = null
                            }}
                            onKeyDown={(event) => onTabKeyDown(event, tab, index)}
                            onClick={() => onActivate(tab, herdrRuntimeTab)}
                        >
                            {tab.kind === "preview" ? (
                                <Globe className="size-[15px] shrink-0" aria-hidden="true" />
                            ) : tab.kind === "herdr-terminal" ? (
                                <Bot className="size-[15px] shrink-0" aria-hidden="true" />
                            ) : tab.kind === "markdown-preview" ? (
                                <FileIcon fileName="preview.md" className="size-[15px] shrink-0" />
                            ) : (
                                <FileIcon fileName={tab.name} className="size-[15px] shrink-0" />
                            )}
                            <span className="max-w-[140px] truncate">{tab.name}</span>
                        </button>
                        {isFileTab(tab) && tab.externallyModified && (
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
                        {isFileTab(tab) && tab.dirty && (
                            <span
                                className="dirty-dot size-[7px] shrink-0 rounded-full bg-[#d68a0c]"
                                title={t("tabBar.unsavedChangesTitle")}
                                aria-hidden="true"
                            />
                        )}
                        {isFileTab(tab) &&
                            (isMarkdownPath(tab.name) || isSvgPath(tab.name)) && (
                            <button
                                type="button"
                                className={
                                    "preview-toggle flex size-[18px] shrink-0 items-center justify-center rounded-[6px] transition-colors " +
                                    ((isMarkdownPath(tab.name)
                                        ? markdownPreviewSources.has(tab.path)
                                        : !svgClosedPaths[tab.path])
                                        ? "bg-(--yz-accent)/16 text-(--yz-accent-ink)"
                                        : "text-(--ink-3) hover:bg-(--paper-3) hover:text-(--ink-0)")
                                }
                                aria-label={t("tabBar.togglePreview", { name: tab.name })}
                                aria-pressed={
                                    isMarkdownPath(tab.name)
                                        ? markdownPreviewSources.has(tab.path)
                                        : !svgClosedPaths[tab.path]
                                }
                                title={
                                    isMarkdownPath(tab.name)
                                        ? t("tabBar.toggleMarkdownPreviewTitle")
                                        : t("tabBar.toggleSvgPreviewTitle")
                                }
                                onClick={(e) => {
                                    e.stopPropagation()
                                    setActiveTab(groupIndex, tab.path)
                                    if (isMarkdownPath(tab.name)) {
                                        toggleMarkdownPreview(tab.path, groupIndex)
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
            <DropdownMenu>
                <DropdownMenuTrigger asChild>
                    <button
                        type="button"
                        data-testid={`tabs-add-menu-${groupIndex}`}
                        aria-label={t("tabBar.addTabMenu")}
                        title={t("tabBar.addTabMenu")}
                        className="ml-[3px] flex size-[28px] shrink-0 items-center justify-center rounded-[9px] text-(--ink-3) transition-colors hover:bg-(--yz-hover) hover:text-(--yz-accent-ink)"
                    >
                        <Plus className="size-[14px]" aria-hidden="true" />
                    </button>
                </DropdownMenuTrigger>
                <DropdownMenuContent align="end" className="w-[260px]">
                    <DropdownMenuGroup>
                        <DropdownMenuItem
                            data-testid="open-browser-tab-menu-item"
                            onSelect={onOpenBrowserTab}
                            className="gap-[9px] px-[9px] py-[7px]"
                        >
                            <Globe className="size-[15px]" aria-hidden="true" />
                            {t("tabBar.openBrowserTab")}
                        </DropdownMenuItem>
                    </DropdownMenuGroup>
                    {showHerdrTabsMenu && (
                        <DropdownMenuGroup>
                            <DropdownMenuItem
                                data-testid="herdr-new-tab-menu-item"
                                disabled={!canCreateHerdrTerminal || !herdrSelectedSpaceId}
                                onSelect={() => void onCreateHerdrTab()}
                                className="gap-[9px] px-[9px] py-[7px]"
                            >
                                <SquareTerminal className="size-[15px]" aria-hidden="true" />
                                {t("tabBar.newHerdrTab")}
                            </DropdownMenuItem>
                            <DropdownMenuLabel>{t("tabBar.existingHerdrTabs")}</DropdownMenuLabel>
                            {visibleHerdrTabs.length === 0 ? (
                                <DropdownMenuItem disabled className="px-[9px] py-[7px]">
                                    {t("tabBar.noHerdrTabs")}
                                </DropdownMenuItem>
                            ) : (
                                visibleHerdrTabs.map((herdrTab) => (
                                    <DropdownMenuItem
                                        key={herdrTab.id}
                                        data-testid={`herdr-open-tab-${herdrTab.id}`}
                                        disabled={!canFocusHerdrTab || !herdrTab.terminalId}
                                        onSelect={() => void onOpenHerdrTab(herdrTab.id)}
                                        className="gap-[9px] px-[9px] py-[7px]"
                                    >
                                        <Bot className="size-[15px]" aria-hidden="true" />
                                        <span className="min-w-0 flex-1">
                                            <span className="block truncate font-medium">{herdrTab.label}</span>
                                            <span className="block text-[10px] text-(--ink-4)">
                                                {t("tabBar.herdrPaneCount", { count: herdrTab.paneCount })}
                                            </span>
                                        </span>
                                        {herdrTab.focused && (
                                            <span className="text-[10px] font-medium text-(--yz-accent-ink)">
                                                {t("tabBar.herdrFocused")}
                                            </span>
                                        )}
                                    </DropdownMenuItem>
                                ))
                            )}
                        </DropdownMenuGroup>
                    )}
                </DropdownMenuContent>
            </DropdownMenu>
        </ScrollArea>
    )
}

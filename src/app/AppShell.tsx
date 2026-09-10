import { memo, useCallback, useEffect, useRef, useState } from "react"
import { useTranslation } from "react-i18next"
import { Database, PanelLeft, PanelLeftOpen, PanelRight, PanelRightOpen, PanelsTopLeft, Search, Server, Settings } from "lucide-react"
import { Button } from "@/components/ui/button"
import { Separator } from "@/components/ui/separator"
import { ResizableHandle, ResizablePanel, ResizablePanelGroup } from "@/components/ui/resizable"
import { watchBrandIcon } from "@/theme/brandIcon"
import { BrandMark } from "@/components/BrandMark"

import { isTauri } from "@/lib/platform"
import { getCurrentWindow } from "@tauri-apps/api/window"

import { type Mode } from "@/app/modes"
import { DatabasePanel } from "@/app/panels/DatabasePanel"
import { EditorPanel } from "@/app/panels/EditorPanel"
import { GitPanel } from "@/app/panels/GitPanel"
import { CommandPalette } from "@/app/workbench/CommandPalette"
import { ContextMenu } from "@/app/workbench/ContextMenu"
import { DiffModal } from "@/workbench/git/DiffModal"
import { ProjectEditorPopover } from "@/app/workbench/ProjectEditorPopover"
import { SpaceAgentSidebar } from "@/app/workbench/SpaceAgentSidebar"
import { WorkspaceToolsPanel, type WorkspaceTool } from "@/app/workbench/WorkspaceToolsPanel"
import { DatabaseNavContent } from "@/app/workbench/DatabaseNavContent"
import { SettingsDialog, type ThemePreference } from "@/app/workbench/SettingsDialog"
import { loadAppearanceSettings, saveAppearanceSettings } from "@/app/workbench/settingsStorage"
import { StatusBar } from "@/app/workbench/StatusBar"
import { useSftpStore } from "@/state/sftpStore"
import { logUserAction } from "@/features/logs/userAction"
import i18n from "@/lib/i18n"
import { showsNativeTrafficLights, shortcutLabel } from "@/lib/platform"
import { confirmDiscardingUnsaved } from "@/lib/unsavedGuard"
import { useUpdateStore } from "@/state/updateStore"
import { contextMenuHandler } from "@/state/contextMenuStore"
import { useUiStore } from "@/state/uiStore"
import { applyAccentPreference, type AccentPreference } from "@/theme/accent"
import { openNewTerminalTab } from "@/terminal/openNewTerminalTab"
import "./workbench/workbench-shell.css"

const DEFAULT_NAV_WIDTH = 288
const MIN_NAV_WIDTH = 256
const MAX_NAV_WIDTH = 420

// Below this window width the nav panel auto-collapses so the editor keeps a
// usable width (VS Code-style progressive disclosure). Density never changes —
// resizing reflows, it doesn't zoom. Acts only on threshold crossings so a
// manual collapse/expand is never fought (see the effect).
const NAV_AUTO_COLLAPSE_WIDTH = 880

// Sidebar motion changes shell geometry, not these surfaces. Stable elements
// keep terminal/editor trees out of that render; their store subscriptions still update.
const editorPanel = <EditorPanel />
const gitPanel = <GitPanel />
const databasePanel = <DatabasePanel />
const databaseNav = <DatabaseNavContent />
const spaceAgentSidebar = <SpaceAgentSidebar />
const statusBar = <StatusBar />
const contextMenu = <ContextMenu />
const projectEditorPopover = <ProjectEditorPopover />
const diffModal = <DiffModal />
const StableWorkspaceToolsPanel = memo(WorkspaceToolsPanel)
const StableSettingsDialog = memo(SettingsDialog)
const StableCommandPalette = memo(CommandPalette)

/**
 * Workbench root layout — design reference §1.1. Owns the chrome-level
 * state (mode / nav collapse / settings / palette / theme) and composes
 * the rail, nav panel, workspace column, terminal drawer and status bar.
 * All 5 modes now render real entry-state content (Task E1 + E2); theme
 * preference drives the `dark` class on <html> (Settings → Appearance).
 */
export function AppShell() {
  useEffect(watchBrandIcon, [])
  const { t } = useTranslation("workbenchShell")
  const mode = useUiStore((s) => s.mode)
  const setMode = useUiStore((s) => s.setMode)
  // Settings open/target is a single source of truth in uiStore so the global
  // openSettings(section?, language?) API (rail avatar, CommandPalette, T11
  // status-bar entry) drives one place instead of chrome-local state.
  const settingsOpen = useUiStore((s) => s.settingsOpen)
  const settingsSection = useUiStore((s) => s.settingsSection)
  const settingsNonce = useUiStore((s) => s.settingsNonce)
  const openSettings = useUiStore((s) => s.openSettings)
  const setSettingsOpen = useUiStore((s) => s.setSettingsOpen)
  // Context menu dispatch (contextMenuStore) lives outside the React tree and
  // can't reach navCollapsed/paletteOpen (local state below) directly — it
  // bumps these nonces instead; the effects further down translate a change
  // into the same local-state update the rail button / ⌘K listener already do.
  const sidebarToggleRequest = useUiStore((s) => s.sidebarToggleRequest)
  const paletteOpenRequest = useUiStore((s) => s.paletteOpenRequest)
  const [navCollapsed, setNavCollapsed] = useState(false)
  const [toolsOpen, setToolsOpen] = useState(() => window.innerWidth >= 1200)
  const [toolsWidth, setToolsWidth] = useState(264)
  const [resizingSidebar, setResizingSidebar] = useState<"spaces" | "tools" | null>(null)
  const [checkoutTool, setCheckoutTool] = useState<WorkspaceTool>("files")
  const [databaseVisited, setDatabaseVisited] = useState(mode === "database")
  const [gitVisited, setGitVisited] = useState(mode === "git")
  const leftToggleRef = useRef<HTMLButtonElement>(null)
  const rightToggleRef = useRef<HTMLButtonElement>(null)
  const lastWorkMode = useRef<Mode>("ade")
  const toolsDragRef = useRef<{ x: number; width: number } | null>(null)
  const previousDualWidth = useRef(window.innerWidth >= 1200)
  const toolsAutoCollapsed = useRef(window.innerWidth < 1200)
  const [paletteOpen, setPaletteOpen] = useState(false)
  const [appearance, setAppearance] = useState(loadAppearanceSettings)
  const { theme, accent, leftSidebarBackground, rightSidebarBackground, botAnimations } = appearance
  const [navWidth, setNavWidth] = useState(DEFAULT_NAV_WIDTH)
  const navDragRef = useRef<{ startX: number; startWidth: number } | null>(null)
  // Whether the current collapse was applied automatically (narrow window) vs.
  // by the user, and the last-seen narrow/wide side, so auto-collapse only fires
  // on threshold crossings and only auto-expand undoes an auto-collapse.
  const navAutoCollapsedRef = useRef(false)
  const prevNarrowRef = useRef<boolean | null>(null)
  // Last nonce value this effect has already reacted to — starts at the
  // current value so mount doesn't fire a spurious toggle/open (settingsNonce
  // pattern above uses the same idea via SettingsDialog's own sync effect).
  const sidebarToggleHandledRef = useRef(sidebarToggleRequest)
  const paletteOpenHandledRef = useRef(paletteOpenRequest)
  // The window starts hidden (tauri.conf `visible: false`) so the native
  // chrome never paints the OS theme before the persisted preference applies.
  const windowShownRef = useRef(false)
  const toolsVisible = toolsOpen && mode !== "database"

  useEffect(() => {
    if (mode === "database") setDatabaseVisited(true)
    else lastWorkMode.current = mode
    if (mode === "git") { setGitVisited(true); setCheckoutTool("git") }
  }, [mode])

  const toggleLeft = () => {
    navAutoCollapsedRef.current = false
    if (navCollapsed && window.innerWidth < 1200) setToolsOpen(false)
    setNavCollapsed(value => !value)
    leftToggleRef.current?.focus()
  }
  const toggleTools = () => {
    toolsAutoCollapsed.current = false
    if (mode === "database") {
      setMode("files")
      setToolsOpen(true)
      if (window.innerWidth < 1200) setNavCollapsed(true)
      rightToggleRef.current?.focus()
      return
    }
    if (!toolsOpen && window.innerWidth < 1200) setNavCollapsed(true)
    setToolsOpen(value => !value)
    rightToggleRef.current?.focus()
  }

  useEffect(() => {
    const resize = () => {
      const wide = window.innerWidth >= 1200
      if (wide === previousDualWidth.current) return
      previousDualWidth.current = wide
      if (!wide) {
        if (document.getElementById("workbench-tools")?.contains(document.activeElement)) rightToggleRef.current?.focus()
        setToolsOpen(open => { if (open) toolsAutoCollapsed.current = true; return false })
      } else if (toolsAutoCollapsed.current) {
        toolsAutoCollapsed.current = false
        setToolsOpen(true)
      }
    }
    window.addEventListener("resize", resize)
    return () => window.removeEventListener("resize", resize)
  }, [])

  const onNavResizePointerDown = (event: React.PointerEvent<HTMLDivElement>) => {
    event.currentTarget.setPointerCapture(event.pointerId)
    setResizingSidebar("spaces")
    navDragRef.current = { startX: event.clientX, startWidth: navWidth }
  }

  const onNavResizePointerMove = (event: React.PointerEvent<HTMLDivElement>) => {
    if (!navDragRef.current) return
    const next = navDragRef.current.startWidth + (event.clientX - navDragRef.current.startX)
    setNavWidth(Math.min(MAX_NAV_WIDTH, Math.max(MIN_NAV_WIDTH, next)))
  }

  const onNavResizePointerUp = (event: React.PointerEvent<HTMLDivElement>) => {
    navDragRef.current = null
    setResizingSidebar(null)
    if (event.currentTarget.hasPointerCapture(event.pointerId)) event.currentTarget.releasePointerCapture(event.pointerId)
  }

  // Reflow, don't zoom: the chrome keeps a fixed density at every window size and
  // the editor absorbs the extra space. The one adaptive move is auto-collapsing
  // the nav on narrow windows — but only when crossing the threshold, and an
  // auto-expand only reverses an auto-collapse, so a manual toggle always wins.
  useEffect(() => {
    const syncNav = () => {
      const narrow = window.innerWidth < NAV_AUTO_COLLAPSE_WIDTH
      if (narrow === prevNarrowRef.current) return
      prevNarrowRef.current = narrow
      if (narrow) {
        if (document.getElementById("workbench-spaces")?.contains(document.activeElement)) leftToggleRef.current?.focus()
        setNavCollapsed((collapsed) => {
          if (collapsed) return collapsed
          navAutoCollapsedRef.current = true
          return true
        })
      } else {
        setNavCollapsed((collapsed) => {
          if (!collapsed || !navAutoCollapsedRef.current) return collapsed
          navAutoCollapsedRef.current = false
          return false
        })
      }
    }

    syncNav()
    window.addEventListener("resize", syncNav)
    return () => window.removeEventListener("resize", syncNav)
  }, [])

  useEffect(() => {
    const root = document.documentElement

    // Native window chrome (border hairline, traffic-light backdrop) must
    // follow the app theme, not the OS — a dark system theme otherwise draws
    // a dark frame around the light UI.
    if (isTauri()) {
      const themed = getCurrentWindow().setTheme(theme === "auto" ? null : theme)
      // Reveal the hidden window only after the native side has applied the
      // theme (awaiting the invoke, not just JS call order), so the very
      // first visible frame carries the persisted theme — no OS-theme
      // titlebar flash. lib.rs holds a 3s failsafe show in case the frontend
      // never reaches this point; show even if setTheme rejected.
      if (!windowShownRef.current) {
        windowShownRef.current = true
        void themed
          .catch(() => {})
          .then(() => getCurrentWindow().show())
          .then(() => useUpdateStore.getState().checkInBackgroundOnce())
          .catch(() => {})
      } else {
        void themed.catch(() => {})
      }
    }

    if (theme === "auto") {
      const media = window.matchMedia("(prefers-color-scheme: dark)")
      const applyFromSystem = () => root.classList.toggle("dark", media.matches)
      applyFromSystem()
      media.addEventListener("change", applyFromSystem)
      return () => media.removeEventListener("change", applyFromSystem)
    }

    root.classList.toggle("dark", theme === "dark")
  }, [theme])

  useEffect(() => {
    applyAccentPreference(accent)
  }, [accent])

  useEffect(() => {
    saveAppearanceSettings(appearance)
  }, [appearance])

  useEffect(() => {
    document.documentElement.dataset.botAnimations = String(botAnimations)
    return () => { delete document.documentElement.dataset.botAnimations }
  }, [botAnimations])

  useEffect(() => {
    const handler = (event: KeyboardEvent) => {
      if (event.key === "`" && event.ctrlKey && !event.metaKey && !event.altKey) {
        event.preventDefault()
        void openNewTerminalTab()
      }
    }
    window.addEventListener("keydown", handler)
    return () => window.removeEventListener("keydown", handler)
  }, [])

  // cmHideSidebar (context menu) → same effect as the rail's manual toggle.
  useEffect(() => {
    if (sidebarToggleRequest === sidebarToggleHandledRef.current) return
    sidebarToggleHandledRef.current = sidebarToggleRequest
    navAutoCollapsedRef.current = false
    leftToggleRef.current?.focus()
    setNavCollapsed((collapsed) => !collapsed)
  }, [sidebarToggleRequest])

  // cmCmdPalette (context menu) → open the command palette, same as ⌘K.
  useEffect(() => {
    if (paletteOpenRequest === paletteOpenHandledRef.current) return
    paletteOpenHandledRef.current = paletteOpenRequest
    setPaletteOpen(true)
  }, [paletteOpenRequest])

  // Window close (Alt+F4 / the title-bar close button). The handler is awaited
  // by @tauri-apps/api's onCloseRequested wrapper and preventDefault() cancels
  // the destroy, so this is where unsaved work gets its last chance — Yuzora
  // keeps no drafts (issue #21).
  //
  // This covers window close only. macOS ⌘Q (and Dock → Quit) goes through
  // NSApp.terminate:, and tao registers no applicationShouldTerminate: — the
  // only delegate method that can cancel a termination — so nothing in the
  // stack ever sees it: tao only hooks applicationWillTerminate:, which lands
  // on RunEvent::Exit, never RunEvent::ExitRequested. There is no frontend or
  // Rust hook that can intercept ⌘Q today; see tauri-apps/tauri#9198 (open)
  // and #12978. Don't add a "⌘Q guard" without first checking those.
  //
  // Registered once (no workspace / dirty state in the deps) because Tauri only
  // prevents the native close while a JS listener is attached: re-binding on
  // every workspace switch would open a window where Alt+F4 bypasses the guard
  // entirely. Both the workspace and the dirty set are read from the store at
  // close time instead.
  useEffect(() => {
    if (!isTauri()) return
    let disposed = false
    let unlisten: (() => void) | null = null

    void getCurrentWindow()
      .onCloseRequested(async (event) => {
        // A rejection here must never escape: the wrapper awaits this handler,
        // so a throw would skip both preventDefault() and destroy() — the
        // window would silently stop closing with no dialog and no error.
        // Erring toward "cancel the close" keeps the buffers alive.
        const proceed = await confirmDiscardingUnsaved({
          title: i18n.t("unsavedDialog.exitTitle", { ns: "menus" }),
          description: i18n.t("unsavedDialog.exitDescription", { ns: "menus" }),
          saveLabel: i18n.t("unsavedDialog.saveAll", { ns: "menus" }),
        }).catch((err) => {
          console.warn("unsaved-changes guard failed:", err)
          return false
        })
        if (!proceed) {
          event.preventDefault()
          return
        }
      })
      .then((nextUnlisten) => {
        if (disposed) {
          nextUnlisten()
          return
        }
        unlisten = nextUnlisten
      })
      .catch(() => {})

    return () => {
      disposed = true
      if (unlisten) unlisten()
    }
  }, [])

  const handleModeChange = useCallback((next: Mode) => {
    setMode(next)
    void logUserAction("mode_change", `Switched to ${next} mode`, { mode: next })
  }, [setMode])

  const handleOpenSettings = useCallback(() => {
    openSettings()
    void logUserAction("settings_open", "Opened settings dialog")
  }, [openSettings])

  const handleThemeChange = useCallback((next: ThemePreference) => {
    setAppearance((current) => ({ ...current, theme: next }))
    void logUserAction("theme_change", `Switched to ${next} theme`, { theme: next })
  }, [])

  const handleAccentChange = useCallback((next: AccentPreference) => {
    setAppearance((current) => ({ ...current, accent: next }))
    void logUserAction("accent_change", `Switched to ${next} accent`, { accent: next })
  }, [])

  const handleSidebarBackgroundChange = useCallback((side: "left" | "right", enabled: boolean) => {
    setAppearance(current => ({
      ...current,
      [side === "left" ? "leftSidebarBackground" : "rightSidebarBackground"]: enabled,
    }))
  }, [])

  const handleBotAnimationsChange = useCallback((enabled: boolean) => {
    setAppearance(current => ({ ...current, botAnimations: enabled }))
  }, [])

  const handleToolChange = useCallback((tool: WorkspaceTool) => {
    setCheckoutTool(tool)
    if (mode === "database" || (mode === "git" && tool === "files")) handleModeChange("files")
  }, [mode, handleModeChange])

  const handleOpenGraph = useCallback(() => {
    useUiStore.getState().setGitPanelTab("log")
    handleModeChange("git")
  }, [handleModeChange])

  // ADE shares the editor surface (mixed file/preview/herdr-terminal pages);
  // keep the shared 44px floor rather than the old AgentZone 280px card floor.
  const mainSurfaceMinHeight = 44
  const nativeTrafficLights = showsNativeTrafficLights()

  return (
    <div
      onContextMenu={contextMenuHandler({ kind: "general" })}
      className="relative flex h-screen w-screen flex-col overflow-hidden font-sans text-[13px] text-(--ink-1)"
      style={{ background: "var(--yz-bg)" }}
    >
      <div className="workbench-window-grip" data-tauri-drag-region title={t("windowDragHint")} />
      <div className="workbench-body" data-resizing={resizingSidebar ?? undefined} data-native-lights={nativeTrafficLights} data-left-collapsed={navCollapsed} data-right-collapsed={!toolsVisible}>
        <Button ref={leftToggleRef} variant="ghost" size="icon-sm" className="workbench-edge-toggle workbench-left-toggle" data-expanded={!navCollapsed} style={{left:navCollapsed ? (nativeTrafficLights ? 102 : 17) : navWidth - 33}} aria-label={t(navCollapsed ? "showSpaces" : "hideSpaces")} title={t(navCollapsed ? "showSpaces" : "hideSpaces")} aria-expanded={!navCollapsed} aria-controls="workbench-spaces" onClick={toggleLeft}>{navCollapsed ? <PanelLeftOpen /> : <PanelLeft />}</Button>
        <Button ref={rightToggleRef} variant="ghost" size="icon-sm" className="workbench-edge-toggle workbench-right-toggle" aria-label={t(toolsVisible ? "hideTools" : "showTools")} title={t(toolsVisible ? "hideTools" : "showTools")} aria-expanded={toolsVisible} aria-controls="workbench-tools" onClick={toggleTools}>{toolsVisible ? <PanelRight /> : <PanelRightOpen />}</Button>
        {nativeTrafficLights && navCollapsed && <span className="workbench-collapsed-native-controls" aria-hidden="true" data-tauri-drag-region />}
        <aside id="workbench-spaces" aria-label={t("sidebar")} aria-hidden={navCollapsed} inert={navCollapsed} data-collapsed={navCollapsed} data-background={leftSidebarBackground} className="workbench-spaces" style={{width:navCollapsed ? 0 : navWidth}}>
          <div className="workbench-spaces-surface" style={{width:navWidth}}>
            <div className="workbench-sidebar-chrome" data-tauri-drag-region>
              {nativeTrafficLights && <span className="workbench-native-controls" aria-hidden="true" />}
              <div className="workbench-title" data-tauri-drag-region>
                <BrandMark className="workbench-logomark" data-tauri-drag-region />
                <strong data-tauri-drag-region>Yuzora</strong>
              </div>
              <span className="workbench-toggle-space" aria-hidden="true" />
            </div>
            <div id="workbench-spaces-content" className="workbench-sidebar-content" aria-hidden={navCollapsed} inert={navCollapsed}>
            <nav className="workbench-sidebar-navigation" aria-label={t("sharedTools")}>
              <Button variant="ghost" className="workbench-sidebar-search" aria-label={t("search")} onClick={() => setPaletteOpen(true)}>
                <Search data-icon="inline-start" /><span>{t("searchShort")}</span><span className="workbench-sidebar-shortcut" aria-hidden="true">{shortcutLabel("mod-k")}</span>
              </Button>
              <Button variant="ghost" className="workbench-sidebar-link" aria-label={t(mode === "database" ? "backToWork" : "workspace")} aria-pressed={mode !== "database"} onClick={() => handleModeChange(lastWorkMode.current)}>
                <PanelsTopLeft data-icon="inline-start" /><span>{t("workspace")}</span>
              </Button>
              <Button variant="ghost" className="workbench-sidebar-link" aria-label={t("database")} aria-pressed={mode === "database"} onClick={() => handleModeChange(mode === "database" ? lastWorkMode.current : "database")}>
                <Database data-icon="inline-start" /><span>{t("database")}</span>
              </Button>
              <Button variant="ghost" className="workbench-sidebar-link" aria-label={t("remoteTools")} onClick={() => useSftpStore.getState().setPanelOpen(true)}>
                <Server data-icon="inline-start" /><span>{t("remoteTools")}</span>
              </Button>
            </nav>
            <Separator className="workbench-sidebar-divider" />
            {spaceAgentSidebar}
            <footer className="workbench-sidebar-footer">
              <Separator className="workbench-sidebar-divider" />
              <Button variant="ghost" className="workbench-sidebar-link" aria-label={t("settings")} onClick={handleOpenSettings}><Settings data-icon="inline-start" /><span>{t("settings")}</span></Button>
            </footer>
            </div>
          </div>
        </aside>
        <div role="separator" tabIndex={navCollapsed ? -1 : 0} aria-hidden={navCollapsed} inert={navCollapsed} data-collapsed={navCollapsed} aria-label={t("resizeSpaces")} aria-orientation="vertical" aria-valuenow={navWidth} aria-valuemin={MIN_NAV_WIDTH} aria-valuemax={MAX_NAV_WIDTH} className="workbench-resize-handle" onPointerDown={onNavResizePointerDown} onPointerMove={onNavResizePointerMove} onPointerUp={onNavResizePointerUp} onPointerCancel={onNavResizePointerUp} onLostPointerCapture={() => {navDragRef.current=null;setResizingSidebar(null)}} onKeyDown={event => {
          const next = event.key === "Home" ? MIN_NAV_WIDTH : event.key === "End" ? MAX_NAV_WIDTH : event.key === "ArrowLeft" ? navWidth-16 : event.key === "ArrowRight" ? navWidth+16 : null
          if(next!==null){event.preventDefault();setNavWidth(Math.min(MAX_NAV_WIDTH,Math.max(MIN_NAV_WIDTH,next)))}
        }}><span /></div>
        <div className="workbench-workspace" data-utility-row={mode === "git" && (navCollapsed || !toolsVisible)}>
          <div data-testid="main-surface" className="workbench-main-surface" style={{minHeight:mainSurfaceMinHeight}}>
            <div hidden={mode!=="files" && mode!=="ade"} inert={mode!=="files" && mode!=="ade"} className="workbench-mode-surface">{editorPanel}</div>
            {(gitVisited || mode === "git") && <div hidden={mode!=="git"} inert={mode!=="git"} className="workbench-mode-surface">{gitPanel}</div>}
            {(databaseVisited || mode === "database") && <div hidden={mode!=="database"} inert={mode!=="database"} className="workbench-database-surface">
              <ResizablePanelGroup orientation="horizontal" className="min-h-0 min-w-0 flex-1">
                <ResizablePanel id="database-navigation" defaultSize="280px" minSize="240px" maxSize="480px" groupResizeBehavior="preserve-pixel-size">
                  <aside aria-label={t("databaseConnections")} className="workbench-database-nav">{databaseNav}</aside>
                </ResizablePanel>
                <ResizableHandle withHandle aria-label={t("resizeDatabase")} className="workbench-database-resize" />
                <ResizablePanel id="database-content" minSize="280px">
                  <div className="workbench-database-main">{databasePanel}</div>
                </ResizablePanel>
              </ResizablePanelGroup>
            </div>}
          </div>
        </div>
        <div role="separator" tabIndex={toolsVisible ? 0 : -1} aria-hidden={!toolsVisible} inert={!toolsVisible} data-collapsed={!toolsVisible} aria-label={t("resizeTools")} aria-orientation="vertical" aria-valuenow={toolsWidth} aria-valuemin={224} aria-valuemax={420} className="workbench-resize-handle" onPointerDown={event => {event.currentTarget.setPointerCapture(event.pointerId);setResizingSidebar("tools");toolsDragRef.current={x:event.clientX,width:toolsWidth}}} onPointerMove={event => {const drag=toolsDragRef.current;if(drag)setToolsWidth(Math.min(420,Math.max(224,drag.width+drag.x-event.clientX)))}} onPointerUp={event => {toolsDragRef.current=null;setResizingSidebar(null);if(event.currentTarget.hasPointerCapture(event.pointerId))event.currentTarget.releasePointerCapture(event.pointerId)}} onPointerCancel={() => {toolsDragRef.current=null;setResizingSidebar(null)}} onLostPointerCapture={() => {toolsDragRef.current=null;setResizingSidebar(null)}} onKeyDown={event => {
          const next=event.key === "Home" ? 224 : event.key === "End" ? 420 : event.key === "ArrowLeft" ? toolsWidth+16 : event.key === "ArrowRight" ? toolsWidth-16 : null
          if(next!==null){event.preventDefault();setToolsWidth(Math.min(420,Math.max(224,next)))}
        }}><span /></div>
        <aside id="workbench-tools" aria-label={t("tools")} aria-hidden={!toolsVisible} inert={!toolsVisible} data-collapsed={!toolsVisible} data-background={rightSidebarBackground} className="workbench-tools" style={{width:toolsVisible ? toolsWidth : 0}}>
          <div className="workbench-tools-surface" style={{width:toolsWidth}}>
            <div className="workbench-sidebar-heading workbench-tools-chrome" data-tauri-drag-region>
              <strong data-tauri-drag-region>{t("tools")}</strong>
              <span className="workbench-toggle-space" aria-hidden="true" />
            </div>
            <div id="workbench-tools-content" className="workbench-sidebar-content" aria-hidden={!toolsVisible} inert={!toolsVisible}>
              <StableWorkspaceToolsPanel tool={checkoutTool} onToolChange={handleToolChange} onOpenGraph={handleOpenGraph} />
            </div>
          </div>
        </aside>
      </div>

      {statusBar}

      <StableCommandPalette
        open={paletteOpen}
        onOpenChange={setPaletteOpen}
        onSelectMode={handleModeChange}
        onOpenSettings={handleOpenSettings}
      />

      <StableSettingsDialog
        open={settingsOpen}
        onOpenChange={setSettingsOpen}
        theme={theme}
        onThemeChange={handleThemeChange}
        accent={accent}
        onAccentChange={handleAccentChange}
        leftSidebarBackground={leftSidebarBackground}
        rightSidebarBackground={rightSidebarBackground}
        onSidebarBackgroundChange={handleSidebarBackgroundChange}
        botAnimations={botAnimations}
        onBotAnimationsChange={handleBotAnimationsChange}
        initialSection={settingsSection ?? undefined}
        openNonce={settingsNonce}
      />

      {contextMenu}
      {projectEditorPopover}

      {/* App-level Diff viewer host (design §D). Renders in-tree (no portal) so
          the overlay's absolute inset-0 covers this relative shell root. Inert
          until the diff modal store opens. */}
      {diffModal}
    </div>
  )
}

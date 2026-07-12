import { useEffect, useRef, useState } from "react"

import { isTauri } from "@/lib/platform"
import { getCurrentWindow } from "@tauri-apps/api/window"

import { type Mode } from "@/app/modes"
import { AgentZonePanel } from "@/app/panels/AgentZonePanel"
import { DatabasePanel } from "@/app/panels/DatabasePanel"
import { EditorPanel } from "@/app/panels/EditorPanel"
import { GitPanel } from "@/app/panels/GitPanel"
import { SshPanel } from "@/app/panels/SshPanel"
import { CommandPalette } from "@/app/workbench/CommandPalette"
import { ContextMenu } from "@/app/workbench/ContextMenu"
import { DiffModal } from "@/workbench/git/DiffModal"
import { ProjectNavPanel } from "@/app/workbench/ProjectNavPanel"
import { SettingsDialog, type ThemePreference } from "@/app/workbench/SettingsDialog"
import { StatusBar } from "@/app/workbench/StatusBar"
import { TerminalDrawer } from "@/app/workbench/TerminalDrawer"
import { WorkspaceRail } from "@/app/workbench/WorkspaceRail"
import { logUserAction } from "@/features/logs/userAction"
import { devServerStopWorkspace, ptyCloseWorkspace } from "@/lib/ipc"
import { showsNativeTrafficLights } from "@/lib/platform"
import { cn } from "@/lib/utils"
import { contextMenuHandler } from "@/state/contextMenuStore"
import { useUiStore } from "@/state/uiStore"
import { PREVIEW_TAB_PATH, useWorkspaceStore } from "@/state/workspaceStore"

const DEFAULT_NAV_WIDTH = 266
const MIN_NAV_WIDTH = 220
const MAX_NAV_WIDTH = 420

// Below this window width the nav panel auto-collapses so the editor keeps a
// usable width (VS Code-style progressive disclosure). Density never changes —
// resizing reflows, it doesn't zoom. Acts only on threshold crossings so a
// manual collapse/expand is never fought (see the effect).
const NAV_AUTO_COLLAPSE_WIDTH = 880

/**
 * Workbench root layout — design reference §1.1. Owns the chrome-level
 * state (mode / nav collapse / settings / palette / theme) and composes
 * the rail, nav panel, workspace column, terminal drawer and status bar.
 * All 5 modes now render real entry-state content (Task E1 + E2); theme
 * preference drives the `dark` class on <html> (Settings → Appearance).
 */
export function AppShell() {
  const mode = useUiStore((s) => s.mode)
  const setMode = useUiStore((s) => s.setMode)
  // Settings open/target is a single source of truth in uiStore so the global
  // openSettings(section?, language?) API (rail avatar, CommandPalette, T11
  // status-bar entry) drives one place instead of chrome-local state.
  const settingsOpen = useUiStore((s) => s.settingsOpen)
  const settingsSection = useUiStore((s) => s.settingsSection)
  const settingsLanguage = useUiStore((s) => s.settingsLanguage)
  const settingsNonce = useUiStore((s) => s.settingsNonce)
  const openSettings = useUiStore((s) => s.openSettings)
  const setSettingsOpen = useUiStore((s) => s.setSettingsOpen)
  const terminalOpen = useUiStore((s) => s.terminalOpen)
  const toggleTerminal = useUiStore((s) => s.toggleTerminal)
  // Context menu dispatch (contextMenuStore) lives outside the React tree and
  // can't reach navCollapsed/paletteOpen (local state below) directly — it
  // bumps these nonces instead; the effects further down translate a change
  // into the same local-state update the rail button / ⌘K listener already do.
  const sidebarToggleRequest = useUiStore((s) => s.sidebarToggleRequest)
  const paletteOpenRequest = useUiStore((s) => s.paletteOpenRequest)
  // Preview is a singleton tab now: the rail's active state = "a preview tab
  // exists somewhere", and its toggle opens/focuses/closes that tab.
  const previewTabExists = useWorkspaceStore((s) =>
    s.groups.some((g) => g.tabs.some((t) => t.path === PREVIEW_TAB_PATH))
  )
  const togglePreviewTab = useWorkspaceStore((s) => s.togglePreviewTab)
  const currentWorkspace = useWorkspaceStore((s) => s.workspacePath)
  const [navCollapsed, setNavCollapsed] = useState(false)
  const [paletteOpen, setPaletteOpen] = useState(false)
  const [theme, setTheme] = useState<ThemePreference>("light")
  const [navWidth, setNavWidth] = useState(DEFAULT_NAV_WIDTH)
  const [navResizing, setNavResizing] = useState(false)
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

  const onNavResizePointerDown = (event: React.PointerEvent<HTMLDivElement>) => {
    event.currentTarget.setPointerCapture(event.pointerId)
    navDragRef.current = { startX: event.clientX, startWidth: navWidth }
    setNavResizing(true)
  }

  const onNavResizePointerMove = (event: React.PointerEvent<HTMLDivElement>) => {
    if (!navDragRef.current) return
    const next = navDragRef.current.startWidth + (event.clientX - navDragRef.current.startX)
    setNavWidth(Math.min(MAX_NAV_WIDTH, Math.max(MIN_NAV_WIDTH, next)))
  }

  const onNavResizePointerUp = (event: React.PointerEvent<HTMLDivElement>) => {
    navDragRef.current = null
    setNavResizing(false)
    event.currentTarget.releasePointerCapture(event.pointerId)
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
    if (isTauri()) void getCurrentWindow().setTheme(theme === "auto" ? null : theme)

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
    const handler = (event: KeyboardEvent) => {
      if (event.key === "`" && event.ctrlKey && !event.metaKey && !event.altKey) {
        event.preventDefault()
        toggleTerminal()
      }
    }
    window.addEventListener("keydown", handler)
    return () => window.removeEventListener("keydown", handler)
  }, [toggleTerminal])

  // cmHideSidebar (context menu) → same effect as the rail's manual toggle.
  useEffect(() => {
    if (sidebarToggleRequest === sidebarToggleHandledRef.current) return
    sidebarToggleHandledRef.current = sidebarToggleRequest
    navAutoCollapsedRef.current = false
    setNavCollapsed((collapsed) => !collapsed)
  }, [sidebarToggleRequest])

  // cmCmdPalette (context menu) → open the command palette, same as ⌘K.
  useEffect(() => {
    if (paletteOpenRequest === paletteOpenHandledRef.current) return
    paletteOpenHandledRef.current = paletteOpenRequest
    setPaletteOpen(true)
  }, [paletteOpenRequest])

  useEffect(() => {
    if (!isTauri() || !currentWorkspace) return
    let disposed = false
    let unlisten: (() => void) | null = null

    void getCurrentWindow()
      .onCloseRequested(() => {
        void ptyCloseWorkspace(currentWorkspace)
        void devServerStopWorkspace(currentWorkspace)
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
  }, [currentWorkspace])

  const handleModeChange = (next: Mode) => {
    setMode(next)
    void logUserAction("mode_change", `Switched to ${next} mode`, { mode: next })
  }

  const handleOpenSettings = () => {
    openSettings()
    void logUserAction("settings_open", "Opened settings dialog")
  }

  const handleThemeChange = (next: ThemePreference) => {
    setTheme(next)
    void logUserAction("theme_change", `Switched to ${next} theme`, { theme: next })
  }

  return (
    <div
      onContextMenu={contextMenuHandler({ kind: "general" })}
      className="relative flex h-screen w-screen flex-col overflow-hidden font-sans text-[13px] text-(--ink-1)"
      style={{ background: "var(--yz-bg)" }}
    >
      {/* Title band along the window top: the whole strip drags the window
          (the overlay title bar's own mouse handling is swallowed by the
          webview). Starts at left-20 (80px) to leave the native traffic
          lights' hit-region (tauri.conf.json trafficLightPosition x:7,y:15,
          buttons span roughly x=7..59) clickable instead of drag-swallowed.
          The content row below shifts down by the same 20px so panels clear
          the buttons. */}
      {showsNativeTrafficLights() && (
        <div aria-hidden="true" data-tauri-drag-region className="absolute left-20 right-0 top-0 z-50 h-[20px]" />
      )}

      <div className={cn("flex min-h-0 flex-1", showsNativeTrafficLights() && "pt-[20px]")}>
        <WorkspaceRail
          navCollapsed={navCollapsed}
          onToggleNav={() => {
            // A manual toggle overrides the automatic narrow-window behaviour.
            navAutoCollapsedRef.current = false
            setNavCollapsed((collapsed) => !collapsed)
          }}
          onOpenSettings={handleOpenSettings}
          previewOpen={previewTabExists}
          onTogglePreview={togglePreviewTab}
          terminalOpen={terminalOpen}
          onToggleTerminalDrawer={toggleTerminal}
        />

        {/* Design reference navStyle: the panel stays mounted and collapses
            via width 280ms spring + opacity 200ms ease-out; inert blocks
            focus/interaction while hidden. Width transition is suppressed
            while the workspace-area resize handle is actively dragging so
            the panel tracks the pointer 1:1 instead of chasing it. */}
        <div
          aria-hidden={navCollapsed}
          inert={navCollapsed}
          className={cn(
            "flex min-h-0 shrink-0 overflow-hidden",
            !navResizing && "transition-[width,opacity] duration-[280ms] ease-(--ease-spring)"
          )}
          style={{
            width: navCollapsed ? 0 : navWidth,
            opacity: navCollapsed ? 0 : 1,
          }}
        >
          <ProjectNavPanel
            mode={mode}
            onModeChange={handleModeChange}
            onOpenPalette={() => setPaletteOpen(true)}
          />
        </div>

        <div className="flex min-w-0 flex-1 pt-[8px] pr-[8px] pb-[8px]">
          {/* Resize handle lives on the workspace side (like TerminalDrawer's
              own drag handle) rather than floating between the two panels.
              Kept mounted (as inert spacing) while the nav is collapsed so
              the workspace area's left inset stays consistent either way. */}
          <div
            onPointerDown={!navCollapsed ? onNavResizePointerDown : undefined}
            onPointerMove={!navCollapsed ? onNavResizePointerMove : undefined}
            onPointerUp={!navCollapsed ? onNavResizePointerUp : undefined}
            title={!navCollapsed ? "Drag to resize" : undefined}
            className={cn(
              "flex w-[10px] shrink-0 items-center justify-center",
              !navCollapsed && "cursor-col-resize"
            )}
          >
            {!navCollapsed && <div className="h-[34px] w-[3px] rounded-full bg-(--line-2)" />}
          </div>

          <div
            className="flex min-w-0 flex-1 flex-col transition-[gap] duration-[280ms] ease-(--ease-spring)"
            style={{ gap: terminalOpen ? 10 : 0 }}
          >
            <div className="flex min-h-0 flex-1 flex-col gap-[10px]">
              {/* EditorPanel stays mounted (CSS-hidden, not unmounted) across mode
                  switches so unsaved edits and undo history survive leaving Files mode. */}
              <div className={mode === "files" ? "contents" : "hidden"}>
                <EditorPanel />
              </div>
              {mode === "git" && <GitPanel />}
              {mode === "database" && <DatabasePanel />}
              {/* SshPanel stays mounted (CSS-hidden) so a live SSH terminal —
                  and its xterm scrollback — survive leaving SSH mode. */}
              <div className={mode === "ssh" ? "contents" : "hidden"}>
                <SshPanel />
              </div>
              {mode === "agent" && <AgentZonePanel />}
            </div>

            <TerminalDrawer visible={terminalOpen} />
          </div>
        </div>
      </div>

      <StatusBar />

      <CommandPalette
        open={paletteOpen}
        onOpenChange={setPaletteOpen}
        onSelectMode={handleModeChange}
        onOpenSettings={handleOpenSettings}
      />

      <SettingsDialog
        open={settingsOpen}
        onOpenChange={setSettingsOpen}
        theme={theme}
        onThemeChange={handleThemeChange}
        initialSection={settingsSection ?? undefined}
        initialLanguage={settingsLanguage ?? undefined}
        openNonce={settingsNonce}
      />

      <ContextMenu />

      {/* App-level Diff viewer host (design §D). Renders in-tree (no portal) so
          the overlay's absolute inset-0 covers this relative shell root. Inert
          until the diff modal store opens. */}
      <DiffModal />
    </div>
  )
}

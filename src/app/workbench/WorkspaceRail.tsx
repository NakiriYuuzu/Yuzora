import { PanelLeft, Plus } from "lucide-react"
import { useEffect, useMemo, useRef, useState } from "react"
import { useTranslation } from "react-i18next"

import { cn } from "@/lib/utils"
import { openWorkspaceAtPath, pickWorkspace } from "@/lib/workspaceActions"
import { selectWorkspaceAgentCounts, useAgentStore } from "@/state/agentStore"
import { contextMenuHandler } from "@/state/contextMenuStore"
import { normalizeWorkspacePath, useRecentWorkspacesStore } from "@/state/recentWorkspaces"
import { useWorkspaceStore } from "@/state/workspaceStore"

interface WorkspaceRailProps {
  navCollapsed: boolean
  onToggleNav: () => void
  onOpenSettings: () => void
  previewOpen: boolean
  onTogglePreview: () => void
  terminalOpen: boolean
  onToggleTerminalDrawer: () => void
}

const RAIL_BUTTON_CLASS =
  "flex h-[32px] w-[38px] items-center justify-center rounded-[10px] transition-all duration-[160ms] ease-(--ease-out) hover:bg-(--yz-hover) hover:text-(--yz-accent-ink)"
const RAIL_IDLE_CLASS = "text-(--ink-3)"
const RAIL_ACTIVE_CLASS = "bg-(--yz-hover) text-(--yz-accent-ink)"

/**
 * Activity rail — design reference 5.1. 60px fixed column: the nav-collapse
 * toggle and terminal/browser dock toggles up top, then the RECENT workspace
 * tiles, then the "open workspace" slot and settings avatar pinned to the
 * bottom. On macOS the native traffic lights float in AppShell's title band
 * above this rail (hidden on other platforms).
 */
export function WorkspaceRail({
  navCollapsed,
  onToggleNav,
  onOpenSettings,
  previewOpen,
  onTogglePreview,
  terminalOpen,
  onToggleTerminalDrawer
}: WorkspaceRailProps) {
  const { t } = useTranslation("workbench")
  const recents = useRecentWorkspacesStore((s) => s.list)
  const workspacePath = useWorkspaceStore((s) => s.workspacePath)
  const activePath = workspacePath ? normalizeWorkspacePath(workspacePath) : null
  const sessions = useAgentStore((s) => s.sessions)
  const agentCounts = useMemo(() => selectWorkspaceAgentCounts(sessions), [sessions])

  const [notice, setNotice] = useState<string | null>(null)
  const noticeTimer = useRef<number | null>(null)

  useEffect(
    () => () => {
      if (noticeTimer.current !== null) clearTimeout(noticeTimer.current)
    },
    []
  )

  function showNotice(message: string) {
    setNotice(message)
    if (noticeTimer.current !== null) clearTimeout(noticeTimer.current)
    noticeTimer.current = window.setTimeout(() => setNotice(null), 4000)
  }

  async function handleOpenRecent(path: string) {
    try {
      await openWorkspaceAtPath(path)
    } catch {
      // Folder moved/deleted — drop it from the MRU and flag it once.
      useRecentWorkspacesStore.getState().remove(path)
      showNotice(t("rail.recentNotFound", { name: folderName(path) }))
    }
  }

  async function handleOpenWorkspace() {
    try {
      await pickWorkspace()
    } catch (e) {
      showNotice(t("rail.openFolderFailed", { error: String(e) }))
    }
  }

  return (
    <nav
      aria-label={t("rail.ariaLabel")}
      onContextMenu={contextMenuHandler({ kind: "rail" })}
      className="flex w-[60px] shrink-0 flex-col items-center gap-[5px] pt-[13px] pb-[11px]"
    >
      <button
        type="button"
        aria-label={t("rail.toggleSidebar")}
        aria-pressed={!navCollapsed}
        title={t("rail.toggleSidebar")}
        onClick={onToggleNav}
        className={cn(
          "flex h-[32px] w-[38px] items-center justify-center rounded-[10px] transition-all duration-[160ms] ease-(--ease-out) hover:bg-(--yz-hover) hover:text-(--yz-accent-ink)",
          navCollapsed ? "text-(--ink-3)" : "bg-(--yz-hover) text-(--yz-accent-ink)"
        )}
      >
        <PanelLeft className="size-[17px]" aria-hidden="true" />
      </button>

      <button
        type="button"
        aria-label={t("rail.toggleTerminal")}
        aria-pressed={terminalOpen}
        title={t("rail.toggleTerminalTitle")}
        onClick={onToggleTerminalDrawer}
        className={cn(RAIL_BUTTON_CLASS, terminalOpen ? RAIL_ACTIVE_CLASS : RAIL_IDLE_CLASS)}
      >
        <svg
          width="16"
          height="16"
          viewBox="0 0 24 24"
          fill="none"
          stroke="currentColor"
          strokeWidth="1.8"
          strokeLinecap="round"
          strokeLinejoin="round"
          aria-hidden="true"
        >
          <path d="m5 8 4 4-4 4M13 16h6" />
        </svg>
      </button>

      <button
        type="button"
        aria-label={t("rail.togglePreview")}
        aria-pressed={previewOpen}
        title={t("rail.togglePreviewTitle")}
        onClick={onTogglePreview}
        className={cn(RAIL_BUTTON_CLASS, previewOpen ? RAIL_ACTIVE_CLASS : RAIL_IDLE_CLASS)}
      >
        <svg
          width="16"
          height="16"
          viewBox="0 0 24 24"
          fill="none"
          stroke="currentColor"
          strokeWidth="1.8"
          strokeLinecap="round"
          strokeLinejoin="round"
          aria-hidden="true"
        >
          <rect x="3" y="4" width="18" height="16" rx="2.5" />
          <path d="M3 9.5h18M6.4 6.9h.01M9 6.9h.01" />
        </svg>
      </button>

      {recents.length > 0 && (
        <>
          <div aria-hidden="true" className="my-[4px] h-px w-[24px] bg-(--line-1)" />
          <div className="text-[9px] font-medium uppercase tracking-[0.12em] text-(--ink-3)">
            {t("rail.recent")}
          </div>
          <div className="flex min-h-0 w-full flex-col items-center gap-[4px] overflow-y-auto">
            {recents.map((path) => {
              const active = activePath === path
              const counts = agentCounts.get(normalizeWorkspacePath(path))
              return (
                <div key={path} className="relative">
                  <button
                    type="button"
                    aria-label={t("rail.openRecentWorkspace", { name: folderName(path) })}
                    aria-pressed={active}
                    title={path}
                    onClick={() => handleOpenRecent(path)}
                    className={cn(
                      "flex size-[34px] shrink-0 items-center justify-center rounded-[10px] text-[13px] font-semibold transition-all duration-[160ms] ease-(--ease-out)",
                      active
                        ? "bg-(--yz-hover) text-(--yz-accent-ink) shadow-(--shadow-xs)"
                        : "border border-(--line-1) text-(--ink-2) hover:bg-(--yz-hover) hover:text-(--yz-accent-ink)"
                    )}
                  >
                    {folderName(path).charAt(0).toUpperCase()}
                  </button>
                  {counts && counts.total > 0 && (
                    <span
                      aria-label={t("rail.agentCount", { total: counts.total, running: counts.running })}
                      className={cn(
                        "absolute -right-[3px] -top-[3px] flex h-[15px] min-w-[15px] items-center justify-center rounded-full px-[3px] text-[9px] font-semibold leading-none text-white",
                        counts.running > 0 ? "bg-(--yz-accent)" : "bg-(--ink-4)"
                      )}
                    >
                      {counts.running > 0 ? `${counts.running}/${counts.total}` : counts.total}
                    </span>
                  )}
                </div>
              )
            })}
          </div>
        </>
      )}

      <div className="flex-1" />

      <button
        type="button"
        aria-label={t("rail.openWorkspace")}
        title={t("rail.openWorkspace")}
        onClick={handleOpenWorkspace}
        className="flex size-[38px] items-center justify-center rounded-[11px] border-[1.5px] border-dashed border-(--line-2) text-(--ink-3) transition-all duration-[180ms] ease-(--ease-spring) hover:border-(--yz-accent)/60 hover:bg-(--yz-hover) hover:text-(--yz-accent-ink)"
      >
        <Plus className="size-[16px]" aria-hidden="true" />
      </button>

      <button
        type="button"
        aria-label={t("rail.settings")}
        title={t("rail.settings")}
        onClick={onOpenSettings}
        className="flex size-[32px] items-center justify-center rounded-full bg-[image:var(--grad-dusk)] text-[12px] font-semibold text-white shadow-(--shadow-sm) transition-transform duration-150 ease-(--ease-spring) hover:scale-[1.08]"
      >
        Y
      </button>

      {notice && (
        <div
          role="status"
          className="fixed bottom-[16px] left-[68px] z-50 max-w-[280px] rounded-[10px] border border-(--line-1) px-[12px] py-[8px] text-[11px] shadow-[var(--shadow-xl)]"
          style={{ background: "var(--danger-soft)", color: "var(--status-d)" }}
        >
          {notice}
        </div>
      )}
    </nav>
  )
}

// path.split("/").pop() would return "" for a normalized ("/a/b" — no
// trailing slash) path, but never for these MRU entries; filter(Boolean)
// only guards the theoretical bare "/" case.
function folderName(path: string): string {
  const parts = path.split("/").filter(Boolean)
  return parts.length > 0 ? parts[parts.length - 1] : path
}

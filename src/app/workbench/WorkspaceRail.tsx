import { GitBranch, PanelLeft, Plus } from "lucide-react"

import { cn } from "@/lib/utils"
import { contextMenuHandler } from "@/state/contextMenuStore"
import { changedPathSet, useGitStore } from "@/state/gitStore"

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
 * toggle on top, then terminal/browser dock toggles, then the "new project"
 * slot and settings avatar pinned to the bottom. On macOS the native traffic
 * lights float in AppShell's title band above this rail (hidden on other
 * platforms).
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
  // Git changed-file count for the rail badge (reference §1.1 amber changed
  // badge). Reads the store directly so it needs no new props; hidden when
  // there's nothing to report.
  const changedCount = useGitStore((s) => changedPathSet(s.status).size)

  return (
    <nav
      aria-label="Workspace rail"
      onContextMenu={contextMenuHandler("rail")}
      className="flex w-[60px] shrink-0 flex-col items-center gap-[5px] pt-[13px] pb-[11px]"
    >
      <button
        type="button"
        aria-label="Toggle sidebar"
        aria-pressed={!navCollapsed}
        title="Toggle sidebar"
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
        aria-label="Toggle terminal"
        aria-pressed={terminalOpen}
        title="Toggle terminal panel"
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
        aria-label="Toggle preview"
        aria-pressed={previewOpen}
        title="Toggle browser preview"
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

      {changedCount > 0 && (
        <div
          aria-label={`${changedCount} changed files`}
          title={`${changedCount} changed files`}
          className="flex h-[22px] items-center gap-[4px] rounded-(--r-pill) bg-(--amber-soft) px-[7px] text-[#9a6512]"
        >
          <GitBranch className="size-[12px]" aria-hidden="true" />
          <span className="font-mono text-[10px] font-semibold">{changedCount}</span>
        </div>
      )}

      <div aria-hidden="true" className="my-[4px] h-px w-[24px] bg-(--line-1)" />

      <div className="flex-1" />

      <button
        type="button"
        aria-label="New project"
        title="New project"
        onClick={() => {
          /* no-op placeholder — project creation lands in a later task */
        }}
        className="flex size-[38px] items-center justify-center rounded-[11px] border-[1.5px] border-dashed border-(--line-2) text-(--ink-3) transition-all duration-[180ms] ease-(--ease-spring) hover:border-(--yz-accent)/60 hover:bg-(--yz-hover) hover:text-(--yz-accent-ink)"
      >
        <Plus className="size-[16px]" aria-hidden="true" />
      </button>

      <button
        type="button"
        aria-label="Settings"
        title="Settings"
        onClick={onOpenSettings}
        className="flex size-[32px] items-center justify-center rounded-full bg-[image:var(--grad-dusk)] text-[12px] font-semibold text-white shadow-(--shadow-sm) transition-transform duration-150 ease-(--ease-spring) hover:scale-[1.08]"
      >
        Y
      </button>
    </nav>
  )
}

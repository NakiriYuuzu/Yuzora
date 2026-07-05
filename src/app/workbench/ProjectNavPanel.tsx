import { SearchIcon } from "lucide-react"

import { cn } from "@/lib/utils"
import { MODES, type Mode } from "@/app/modes"
import { AgentNavContent } from "@/app/workbench/AgentNavContent"
import { DatabaseNavContent } from "@/app/workbench/DatabaseNavContent"
import { FilesNavContent } from "@/app/workbench/FilesNavContent"
import { GitNavContent } from "@/app/workbench/GitNavContent"
import { SshNavContent } from "@/app/workbench/SshNavContent"

interface ProjectNavPanelProps {
  mode: Mode
  onModeChange: (mode: Mode) => void
  onOpenPalette: () => void
}

/**
 * Project nav panel — design reference 5.2. Fills the resizable nav column
 * (width owned by AppShell's drag handle) as a card: project header, command
 * field trigger, 5-way mode switcher, and a per-mode content area. Every mode
 * now has a real empty state (Task E1 + E2).
 */
export function ProjectNavPanel({ mode, onModeChange, onOpenPalette }: ProjectNavPanelProps) {
  return (
    <aside
      aria-label="Project navigation"
      className="my-[8px] flex w-full shrink-0 flex-col rounded-(--r-lg) border border-(--line-1) bg-(--yz-glass) shadow-(--shadow-sm) backdrop-blur-[20px] backdrop-saturate-[1.5]"
    >
      <div className="flex items-center gap-[10px] px-[15px] pt-[15px] pb-[12px]">
        <div
          aria-hidden="true"
          className="flex size-[30px] shrink-0 items-center justify-center rounded-[10px] bg-[image:var(--grad-sunrise)] text-[12px] font-semibold text-white"
        >
          Y
        </div>
        <div className="min-w-0 flex-1">
          <p className="truncate font-serif text-[20px] font-medium text-(--ink-1)">yuzora</p>
          <p className="truncate text-[11px] text-(--ink-3)">~/App/Tauri/yuzora</p>
        </div>
      </div>

      <div className="px-[13px] pb-[12px]">
        <button
          type="button"
          onClick={onOpenPalette}
          className="flex h-[38px] w-full items-center gap-[9px] rounded-[12px] border border-(--line-2) bg-(--yz-field) px-[11px] text-left text-(--ink-3) shadow-(--shadow-xs) transition-colors duration-[160ms] ease-(--ease-out) hover:bg-(--yz-solid)"
        >
          <SearchIcon className="size-[15px] shrink-0" aria-hidden="true" />
          <span className="flex-1 truncate text-[13px] font-medium">Search or run a command</span>
          <kbd className="shrink-0 rounded-[6px] bg-(--yz-active) px-[6px] py-[2px] font-mono text-[10.5px] text-(--ink-3)">
            ⌘K
          </kbd>
        </button>
      </div>

      <div role="tablist" aria-label="Workbench mode" className="flex gap-[3px] px-[13px] pb-[11px]">
        {MODES.map((m) => {
          const isActive = m.id === mode
          const Icon = m.icon
          return (
            <button
              key={m.id}
              type="button"
              role="tab"
              aria-selected={isActive}
              aria-label={m.label}
              title={m.label}
              onClick={() => onModeChange(m.id)}
              className={cn(
                "flex h-[34px] flex-1 items-center justify-center rounded-[10px] text-(--ink-3) transition-all duration-150 ease-(--ease-out)",
                isActive
                  ? "bg-(--yz-solid) text-(--yz-accent-ink) shadow-(--shadow-xs)"
                  : "hover:bg-(--yz-hover) hover:text-(--ink-1)"
              )}
            >
              <Icon className="size-[17px]" aria-hidden="true" />
            </button>
          )
        })}
      </div>

      {/* Design reference panel header: uppercase active-mode label above the
          per-mode body (the mono hint next to it needs per-mode data that
          doesn't exist yet). */}
      <div className="flex items-center gap-[8px] px-[16px] pb-[6px]">
        <span className="text-[10px] font-semibold tracking-[0.09em] text-(--ink-3) uppercase">
          {MODES.find((m) => m.id === mode)?.label}
        </span>
      </div>

      <div className="min-h-0 flex-1 overflow-y-auto px-[9px] pb-[8px]">
        {mode === "files" && (
          <div data-testid="nav-mode-content-files" className="h-full min-h-[120px]">
            <FilesNavContent />
          </div>
        )}
        {mode === "git" && (
          <div data-testid="nav-mode-content-git" className="h-full min-h-[120px]">
            <GitNavContent />
          </div>
        )}
        {mode === "database" && (
          <div data-testid="nav-mode-content-database" className="h-full min-h-[120px]">
            <DatabaseNavContent />
          </div>
        )}
        {mode === "ssh" && (
          <div data-testid="nav-mode-content-ssh" className="h-full min-h-[120px]">
            <SshNavContent />
          </div>
        )}
        {mode === "agent" && (
          <div data-testid="nav-mode-content-agent" className="h-full min-h-[120px]">
            <AgentNavContent />
          </div>
        )}
      </div>
    </aside>
  )
}

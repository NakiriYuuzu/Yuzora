import { ArrowLeft, ArrowRight, MonitorPlay, RotateCw, Smartphone } from "lucide-react"

import { EmptyState } from "@/app/workbench/EmptyState"
import { suppressContextMenu } from "@/state/contextMenuStore"

/**
 * Docked preview panel — entry state for the plan's "docked browser panel".
 * No design-reference subsection (§5.1–5.10) covers this panel's own chrome,
 * so header control sizing follows the neighboring tab-bar/terminal-header
 * scale. No real navigation or dev-server detection — controls are inert.
 */
export function PreviewPanel() {
  return (
    <div onContextMenu={suppressContextMenu} className="flex min-h-0 flex-1 flex-col">
      <div className="flex h-[38px] shrink-0 items-center gap-[6px] border-b border-(--line-1) px-[8px]">
        <button
          type="button"
          disabled
          aria-label="Back"
          className="flex size-[24px] shrink-0 cursor-not-allowed items-center justify-center rounded-[7px] text-(--ink-4)"
        >
          <ArrowLeft className="size-[14px]" aria-hidden="true" />
        </button>
        <button
          type="button"
          disabled
          aria-label="Forward"
          className="flex size-[24px] shrink-0 cursor-not-allowed items-center justify-center rounded-[7px] text-(--ink-4)"
        >
          <ArrowRight className="size-[14px]" aria-hidden="true" />
        </button>
        <button
          type="button"
          disabled
          aria-label="Reload"
          className="flex size-[24px] shrink-0 cursor-not-allowed items-center justify-center rounded-[7px] text-(--ink-4)"
        >
          <RotateCw className="size-[13px]" aria-hidden="true" />
        </button>

        <input
          readOnly
          aria-label="Preview URL"
          value="http://localhost:—"
          className="h-[24px] min-w-0 flex-1 rounded-[7px] border border-(--line-1) bg-(--yz-sunk) px-[8px] font-mono text-[11px] text-(--ink-3)"
        />

        <button
          type="button"
          disabled
          aria-label="Toggle responsive frame"
          className="flex size-[24px] shrink-0 cursor-not-allowed items-center justify-center rounded-[7px] text-(--ink-4)"
        >
          <Smartphone className="size-[13px]" aria-hidden="true" />
        </button>

        <span className="shrink-0 rounded-[6px] bg-(--yz-sunk) px-[7px] py-[3px] text-[10.5px] text-(--ink-3)">
          No dev server
        </span>
      </div>

      <div className="flex min-h-0 flex-1 items-center justify-center">
        <EmptyState
          icon={MonitorPlay}
          title="Start or connect to a dev server"
          description="The preview will appear here once a server is running."
        />
      </div>
    </div>
  )
}

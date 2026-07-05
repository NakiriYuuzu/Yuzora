import { Bot, SendHorizontal } from "lucide-react"

import { EmptyState } from "@/app/workbench/EmptyState"
import { contextMenuHandler } from "@/state/contextMenuStore"

/**
 * AgentZone mode main region — design reference 5.8. Entry state only: no
 * transcript, tool/diff/permission blocks or slash popup (out of scope).
 * The composer shell's textarea/send button are fully disabled; the
 * violet border is the design's fixed AgentZone identity color (reference
 * §5.8 / §8 gap 12 — the one spot that hardcodes a focus color instead of
 * the shared accent). Real focus can't fire on disabled controls, so the
 * violet is shown as a static resting tint instead.
 */
export function AgentZonePanel() {
  return (
    <div
      onContextMenu={contextMenuHandler("agent")}
      className="yz-modein flex min-h-0 flex-1 flex-col overflow-hidden rounded-(--r-lg) border border-(--line-1) bg-(--paper-0) shadow-(--shadow-lg)"
    >
      <div className="flex min-h-0 flex-1 items-center justify-center">
        <EmptyState
          icon={Bot}
          title="ACP sessions will be managed here"
          description="Connect an agent to start a session."
        />
      </div>

      <div className="flex shrink-0 items-center gap-[8px] border-t border-(--line-1) px-[14px] py-[11px]">
        <textarea
          disabled
          rows={1}
          aria-label="Message the agent"
          placeholder="Message an agent…"
          className="h-[38px] min-w-0 flex-1 resize-none rounded-[10px] border border-[#7b5bff]/30 bg-(--yz-field) px-[12px] py-[9px] text-[13px] text-(--ink-3) placeholder:text-(--ink-4) disabled:cursor-not-allowed"
        />
        <button
          type="button"
          disabled
          aria-label="Send message"
          className="flex size-[38px] shrink-0 items-center justify-center rounded-[10px] bg-(--ink-1) text-(--paper-0) disabled:cursor-not-allowed disabled:opacity-50"
        >
          <SendHorizontal className="size-[16px]" aria-hidden="true" />
        </button>
      </div>
    </div>
  )
}

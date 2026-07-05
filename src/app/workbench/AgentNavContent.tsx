import { MessagesSquare } from "lucide-react"

import { DashedActionButton } from "@/app/workbench/DashedActionButton"
import { EmptyState } from "@/app/workbench/EmptyState"

/**
 * AgentZone mode nav content — empty sessions list. Reference §7's
 * AgentZone section covers an active session's vocabulary (running/
 * completed/waiting/failed) but not an empty-list state, so this follows
 * the Files/Database/SSH nav pattern.
 */
export function AgentNavContent() {
  return (
    <div className="flex h-full flex-col gap-[10px]">
      <div className="flex flex-1 items-center justify-center">
        <EmptyState
          icon={MessagesSquare}
          title="No sessions yet"
          description="Start a new ACP session to connect an agent"
        />
      </div>
      <DashedActionButton
        label="New session"
        onClick={() => {
          /* no-op placeholder — ACP session creation lands in a later task */
        }}
      />
    </div>
  )
}

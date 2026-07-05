import { Database } from "lucide-react"

import { DashedActionButton } from "@/app/workbench/DashedActionButton"
import { EmptyState } from "@/app/workbench/EmptyState"

/**
 * Database mode nav content — no configured connections. Reference §7's
 * Database section only covers an already-connected state (connected pill,
 * Tables list), so the empty-state copy follows the Files/Git nav pattern.
 */
export function DatabaseNavContent() {
  return (
    <div className="flex h-full flex-col gap-[10px]">
      <div className="flex flex-1 items-center justify-center">
        <EmptyState
          icon={Database}
          title="No database connections"
          description="Add a connection to browse its tables"
        />
      </div>
      <DashedActionButton
        label="New connection"
        onClick={() => {
          /* no-op placeholder — connection setup lands in a later task */
        }}
      />
    </div>
  )
}

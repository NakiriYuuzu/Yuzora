import { Table2 } from "lucide-react"

import { EmptyState } from "@/app/workbench/EmptyState"

/**
 * Database mode main region — entry state only. No design-reference
 * subsection covers Database (§8 gap notes it extends the SQL console
 * empty-state baseline / §4 component table), so this is a single card
 * with an empty state — no fake tables, connections or query results.
 */
export function DatabasePanel() {
  return (
    <div className="yz-modein flex min-h-0 flex-1 items-center justify-center rounded-(--r-lg) border border-(--line-1) bg-(--paper-0) shadow-(--shadow-lg)">
      <EmptyState
        icon={Table2}
        title="Database connections are not configured"
        description="Add a connection to browse tables and run queries here."
      />
    </div>
  )
}

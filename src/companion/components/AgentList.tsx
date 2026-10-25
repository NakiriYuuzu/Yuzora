import { ChevronRight } from "lucide-react"
import type { AgentRow } from "../lib/agents"
import { open } from "../store"
import { StatusBadge } from "./StatusBadge"

/** A Space's agents, each a tap away from its pane. */
export function AgentList({ agents }: { agents: AgentRow[] }) {
  return (
    <ul>
      {agents.map((agent) => (
        <li key={agent.paneId} className="border-b last:border-b-0">
          <button
            type="button"
            className="flex min-h-14 w-full items-center gap-3 px-3 py-2 text-left active:bg-muted"
            onClick={() => open({ name: "pane", session: agent.session, pane: agent.paneId })}
          >
            <span className="min-w-0 flex-1">
              <span className="block truncate text-base font-medium">{agent.name}</span>
              <span className="block truncate text-xs text-muted-foreground">{[agent.kind, agent.tabLabel, agent.hint ?? agent.paneLabel].filter(Boolean).join(" · ")}</span>
            </span>
            <StatusBadge status={agent.status} />
            <ChevronRight className="size-4 shrink-0 text-muted-foreground" aria-hidden />
          </button>
        </li>
      ))}
    </ul>
  )
}

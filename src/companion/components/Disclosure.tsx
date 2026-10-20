import { useState, type ReactNode } from "react"
import { ChevronRight } from "lucide-react"
import { cn } from "@/lib/utils"

/** A card that starts folded and shows `count` next to its title. */
export function Disclosure({ title, count, children }: { title: string; count: number; children: ReactNode }) {
  const [expanded, setExpanded] = useState(false)
  return (
    <div className="overflow-hidden rounded-xl border bg-card">
      <button
        type="button"
        aria-expanded={expanded}
        className="flex min-h-11 w-full items-center gap-2 px-3 py-2 text-left text-sm font-semibold active:bg-muted"
        onClick={() => setExpanded((value) => !value)}
      >
        <ChevronRight className={cn("size-4 shrink-0 text-muted-foreground transition-transform", expanded && "rotate-90")} aria-hidden />
        <span className="min-w-0 flex-1 truncate">{title}</span>
        <span className="text-xs font-normal text-muted-foreground">{count}</span>
      </button>
      {expanded && <div className="border-t">{children}</div>}
    </div>
  )
}

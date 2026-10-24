import { useState, type ReactNode } from "react"
import { ChevronRight } from "lucide-react"
import { Collapsible, CollapsibleContent, CollapsibleTrigger } from "@/components/ui/collapsible"
import { cn } from "@/lib/utils"

/** A card that starts folded and shows `count` next to its title. */
export function Disclosure({ title, count, children }: { title: string; count: number; children: ReactNode }) {
  const [expanded, setExpanded] = useState(false)
  return (
    <Collapsible open={expanded} onOpenChange={setExpanded} className="overflow-hidden rounded-xl border bg-card">
      <CollapsibleTrigger className="flex min-h-11 w-full items-center gap-2 px-3 py-2 text-left text-sm font-semibold active:bg-muted">
        <ChevronRight className={cn("size-4 shrink-0 text-muted-foreground transition-transform", expanded && "rotate-90")} aria-hidden />
        <span className="min-w-0 flex-1 truncate">{title}</span>
        <span className="text-xs font-normal text-muted-foreground">{count}</span>
      </CollapsibleTrigger>
      <CollapsibleContent className="border-t">{children}</CollapsibleContent>
    </Collapsible>
  )
}

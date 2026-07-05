import type { LucideIcon } from "lucide-react"

import { cn } from "@/lib/utils"

interface EmptyStateProps {
  icon: LucideIcon
  title: string
  description: string
  tone?: "default" | "terminal"
}

/**
 * Shared empty-state layout — design reference §8 gap 8 (no upstream spec
 * for empty states; extends the SQL console baseline): centered icon + copy,
 * 26–28px icon in --ink-3/4, 12.5–13px text. `tone="terminal"` swaps to the
 * --term-* palette for use inside the terminal drawer.
 */
export function EmptyState({ icon: Icon, title, description, tone = "default" }: EmptyStateProps) {
  const isTerminal = tone === "terminal"

  return (
    <div className="flex flex-col items-center gap-[10px] px-4 text-center">
      <Icon
        className={cn("size-[27px]", isTerminal ? "text-(--term-fg2)" : "text-(--ink-3)")}
        aria-hidden="true"
      />
      <div className="flex flex-col gap-[3px]">
        <p className={cn("text-[13px] font-medium", isTerminal ? "text-(--term-fg)" : "text-(--ink-2)")}>
          {title}
        </p>
        <p className={cn("text-[12.5px]", isTerminal ? "text-(--term-fg2)" : "text-(--ink-3)")}>
          {description}
        </p>
      </div>
    </div>
  )
}

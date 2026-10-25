import { useTranslation } from "react-i18next"
import { cn } from "@/lib/utils"
import type { AgentStatus } from "../lib/protocol"

// Same colors as the desktop sidebar (space-agent-sidebar.css).
const TONE: Record<AgentStatus, string> = {
  working: "text-[var(--yz-run)] bg-[color-mix(in_srgb,var(--yz-run)_12%,transparent)]",
  blocked: "text-destructive bg-[color-mix(in_srgb,var(--destructive)_12%,transparent)]",
  done: "text-[var(--term-ok)] bg-[color-mix(in_srgb,var(--term-ok)_12%,transparent)]",
  idle: "text-muted-foreground bg-muted",
  unknown: "text-muted-foreground bg-muted",
}

export function StatusBadge({ status, className }: { status: AgentStatus; className?: string }) {
  const { t } = useTranslation()
  return (
    <span data-status={status} className={cn("inline-flex h-6 shrink-0 items-center rounded-full px-2.5 text-xs font-medium", TONE[status], className)}>
      {t(`status.${status}`)}
    </span>
  )
}

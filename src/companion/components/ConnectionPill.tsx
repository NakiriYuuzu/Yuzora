import { useTranslation } from "react-i18next"
import { cn } from "@yuzora/shared/lib/utils"
import { retryNow } from "../client"
import { useCompanion } from "../store"

export function ConnectionPill() {
  const { t } = useTranslation()
  const state = useCompanion((store) => store.socket)
  if (state === "ready") return null
  return (
    <button
      type="button"
      onClick={retryNow}
      className={cn("h-8 shrink-0 rounded-full px-3 text-xs font-medium", state === "waiting" ? "bg-[color-mix(in_srgb,var(--term-amber)_16%,transparent)]" : "bg-muted text-muted-foreground")}
    >
      {t(`connection.${state}`)}
    </button>
  )
}

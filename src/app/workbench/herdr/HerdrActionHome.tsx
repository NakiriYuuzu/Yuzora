import { useTranslation } from "react-i18next"
import { Bell, ChevronRight, SquareTerminal, type LucideIcon } from "lucide-react"
import { herdrNativeAvailability, herdrTaskAvailability, type HerdrAvailability } from "@/lib/herdrActions"
import type { HerdrSessionRuntime } from "@/lib/herdrTypes"
import type { HerdrTask } from "@/state/herdrToolsStore"
import { Button } from "@/components/ui/button"
import { ReasonNote } from "./controls"
import { taskIcons } from "./taskIcons"

const cards: { id: HerdrTask | "native"; icon: LucideIcon }[] = [
  ...(["worktree", "startAgent", "messageAgent", "movePane", "sessions"] as const).map(id => ({ id, icon: taskIcons[id] })),
  { id: "native", icon: SquareTerminal },
]

/** Task-oriented launcher: what do you want to do, and why is something unavailable. */
export function HerdrActionHome({ runtime, starting, busy, onTask, onNative, onNotifications }: {
  runtime: HerdrSessionRuntime | undefined; starting: boolean; busy: boolean
  onTask: (task: HerdrTask) => void; onNative: () => void; onNotifications: () => void
}) {
  const { t } = useTranslation("herdrTools")
  const availability = (id: HerdrTask | "native"): HerdrAvailability => id === "native" ? herdrNativeAvailability(runtime, starting) : herdrTaskAvailability(id, runtime, starting)
  return <div className="flex min-w-0 flex-col gap-5">
    <div className="flex min-w-0 items-baseline justify-between gap-3">
      <h3 className="text-base font-medium">{t("homeQuestion")}</h3>
      <p className="shrink-0 text-xs text-muted-foreground">{t("homePaletteHint")}</p>
    </div>
    <div role="group" aria-label={t("homeQuestion")} className="grid min-w-0 gap-3 sm:grid-cols-2 lg:grid-cols-3">
      {cards.map(({ id, icon: Icon }) => {
        const state = availability(id)
        return <button key={id} type="button" data-task-card={id} disabled={busy || !state.ok} onClick={() => id === "native" ? onNative() : onTask(id)}
          className="flex min-h-28 min-w-0 flex-col items-start gap-1.5 rounded-xl border bg-card p-3.5 text-left outline-none transition-colors hover:bg-muted/50 focus-visible:border-ring focus-visible:ring-3 focus-visible:ring-ring/50 disabled:cursor-not-allowed disabled:opacity-60 disabled:hover:bg-card">
          <span className="flex size-8 items-center justify-center rounded-lg bg-primary/10 text-primary"><Icon className="size-4" aria-hidden="true" /></span>
          <span className="text-sm font-medium">{t(`tasks.${id}.title`)}</span>
          <span className="text-xs text-muted-foreground [overflow-wrap:anywhere]">{t(`tasks.${id}.description`)}</span>
          {!state.ok && <ReasonNote reason={state.reason} />}
        </button>
      })}
    </div>
    <div className="flex min-w-0 flex-wrap items-center gap-2 border-t pt-4">
      <span className="mr-1 text-xs font-medium text-muted-foreground">{t("advancedRow")}</span>
      {(["integrations", "plugins"] as const).map(id => {
        const Icon = taskIcons[id]
        const state = availability(id)
        return <Button key={id} variant="outline" size="sm" disabled={busy || !state.ok} title={state.ok ? undefined : state.reason} onClick={() => onTask(id)}><Icon data-icon="inline-start" />{t(`tasks.${id}.title`)}</Button>
      })}
      <Button variant="outline" size="sm" disabled={busy} onClick={onNotifications}><Bell data-icon="inline-start" />{t("notificationSettings")}<ChevronRight data-icon="inline-end" /></Button>
    </div>
  </div>
}

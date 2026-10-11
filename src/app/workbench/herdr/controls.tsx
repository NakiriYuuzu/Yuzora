import { useId, type ComponentType, type ReactNode } from "react"
import { useTranslation } from "react-i18next"
import { ChevronDown, ChevronRight, TerminalSquare, TriangleAlert } from "lucide-react"
import { Button } from "@/components/ui/button"
import { Collapsible, CollapsibleContent, CollapsibleTrigger } from "@/components/ui/collapsible"
import { Field, FieldContent, FieldDescription, FieldLabel, FieldTitle } from "@/components/ui/field"
import { Input } from "@/components/ui/input"
import { RadioGroup, RadioGroupItem } from "@/components/ui/radio-group"
import { DropdownMenu, DropdownMenuContent, DropdownMenuLabel, DropdownMenuRadioGroup, DropdownMenuRadioItem, DropdownMenuTrigger } from "@/components/ui/dropdown-menu"
import type { HerdrSnapshot } from "@/lib/herdrTypes"
import { cn } from "@/lib/utils"

export function TextField({ label, value, onChange, hint, error, ...props }: {
  label: string; value: string; onChange: (value: string) => void; hint?: string
  /** Inline validation message; shown as text so it is readable without hovering a native `pattern` tooltip. */
  error?: string | null
} & Omit<React.ComponentProps<typeof Input>, "value" | "onChange">) {
  const id = useId()
  return <Field><FieldLabel htmlFor={id}>{label}</FieldLabel><Input id={id} value={value} onChange={event => onChange(event.target.value)} aria-invalid={error ? true : undefined} aria-describedby={error ? `${id}-error` : undefined} {...props} />
    {error ? <p id={`${id}-error`} role="alert" className="text-xs text-destructive">{error}</p> : hint && <FieldDescription>{hint}</FieldDescription>}</Field>
}

/** Why an action cannot run, shown as visible text rather than only a disabled button. */
export function ReasonNote({ reason, className }: { reason?: string | null; className?: string }) {
  return reason ? <p role="note" className={cn("flex items-start gap-1.5 text-xs text-amber-600 dark:text-amber-400", className)}><TriangleAlert className="mt-px size-3.5 shrink-0" aria-hidden="true" /><span className="min-w-0 [overflow-wrap:anywhere]">{reason}</span></p> : null
}

/** Collapsed-by-default section for rarely needed parameters. */
export function Advanced({ title, children }: { title: string; children: ReactNode }) {
  return <Collapsible className="min-w-0 rounded-lg border">
    <CollapsibleTrigger className="group flex w-full items-center gap-1.5 px-3 py-2 text-left text-xs font-medium text-muted-foreground outline-none hover:text-foreground focus-visible:ring-2 focus-visible:ring-ring/50">
      <ChevronRight className="size-3.5 transition-transform group-data-[state=open]:rotate-90" aria-hidden="true" />{title}
    </CollapsibleTrigger>
    <CollapsibleContent><div className="flex min-w-0 flex-col gap-3 border-t px-3 py-3">{children}</div></CollapsibleContent>
  </Collapsible>
}

/** Numbered step so a multi-part action reads top to bottom. */
export function ToolStep({ index, title, children, aside }: { index: number; title: string; children: ReactNode; aside?: ReactNode }) {
  return <section className="flex min-w-0 flex-col gap-2.5">
    <div className="flex min-w-0 items-center gap-2">
      <span aria-hidden="true" className="flex size-5 shrink-0 items-center justify-center rounded-full bg-primary/10 text-[11px] font-semibold text-primary">{index}</span>
      <h4 className="min-w-0 flex-1 text-[13px] font-medium">{title}</h4>
      {aside}
    </div>
    <div className="min-w-0 pl-7">{children}</div>
  </section>
}

export interface ChoiceOption { value: string; title: ReactNode; description?: ReactNode; icon?: ComponentType<{ className?: string }>; disabled?: boolean }

/** shadcn choice cards: the whole card is the radio label. */
export function ChoiceCards({ label, value, onChange, options, disabled, columns = 1 }: {
  label: string; value: string; onChange: (value: string) => void; options: ChoiceOption[]; disabled?: boolean; columns?: 1 | 2 | 3
}) {
  const id = useId()
  return <RadioGroup aria-label={label} value={value} onValueChange={onChange} disabled={disabled} className={cn("gap-2", columns === 3 ? "sm:grid-cols-3" : columns === 2 ? "sm:grid-cols-2" : "grid-cols-1")}>
    {options.map(option => {
      const Icon = option.icon
      return <FieldLabel key={option.value} htmlFor={`${id}-${option.value}`} className="cursor-pointer has-data-[state=checked]:border-primary/40 has-data-[state=checked]:bg-primary/5 has-[:disabled]:cursor-not-allowed has-[:disabled]:opacity-50">
        <Field orientation="horizontal" className="items-start gap-2.5">
          {Icon && <Icon className="mt-0.5 size-4 shrink-0 text-muted-foreground" />}
          <FieldContent className="min-w-0 gap-0.5">
            <FieldTitle className="min-w-0 max-w-full [overflow-wrap:anywhere]">{option.title}</FieldTitle>
            {option.description && <FieldDescription className="text-xs [overflow-wrap:anywhere]">{option.description}</FieldDescription>}
          </FieldContent>
          <RadioGroupItem id={`${id}-${option.value}`} value={option.value} disabled={option.disabled} className="mt-0.5" />
        </Field>
      </FieldLabel>
    })}
  </RadioGroup>
}

/** Pane picker shown as readable rows instead of a select of raw pane ids. */
export function PaneChoices({ snapshot, value, onChange, disabled, preferred, onlyPanes, agentAware }: {
  snapshot: HerdrSnapshot | null; value: string; onChange: (value: string) => void; disabled?: boolean
  /** The pane the entry point selected; marked so the choice is never hidden state. */
  preferred?: string
  /** Restrict to these pane ids (e.g. panes that run an agent). */
  onlyPanes?: string[]
  /** Mark panes that already run an agent and list them after free panes. */
  agentAware?: boolean
}) {
  const { t } = useTranslation("herdrTools")
  const agents = new Map(snapshot?.agents.filter(agent => agent.paneId).map(agent => [agent.paneId!, agent]))
  const panes = (snapshot?.terminals.filter(pane => pane.paneId && (!onlyPanes || onlyPanes.includes(pane.paneId))) ?? [])
    .map((pane, index) => ({ pane, index, busy: agentAware && agents.has(pane.paneId!) }))
    .sort((a, b) => Number(a.busy) - Number(b.busy) || a.index - b.index)
    .map(item => item.pane)
  if (!panes.length) return <p className="text-xs text-muted-foreground">{t("noPanes")}</p>
  const where = (workspaceId?: string | null, tabId?: string | null) => [
    snapshot?.spaces.find(space => space.id === workspaceId)?.label,
    snapshot?.tabs.find(tab => tab.id === tabId)?.label,
  ].filter(Boolean).join(" · ")
  const badge = "ml-1.5 rounded-full bg-muted px-1.5 text-[10px] font-normal text-muted-foreground"
  return <ChoiceCards label={t("pane")} value={value} onChange={onChange} disabled={disabled} columns={2} options={panes.map(pane => {
    const agent = agents.get(pane.paneId!)
    return {
      value: pane.paneId!,
      icon: TerminalSquare,
      title: <>{pane.title ?? pane.terminalId}
        {pane.paneId === preferred && <span className={cn(badge, "bg-primary/10 text-primary")}>{t("selectedPane")}</span>}
        {pane.paneId === snapshot?.focusedPaneId && <span className={badge}>{t("focusedPane")}</span>}
        {agentAware && agent && <span className={badge}>{t("hasAgent", { name: agent.name })}</span>}</>,
      description: <span className="font-mono">{[where(pane.workspaceId, pane.tabId), pane.paneId].filter(Boolean).join(" · ")}</span>,
    }
  })} />
}

/** Compact breadcrumb segment for choosing the host, Session or Space the tools act on. */
export function ScopeMenu({ label, icon: Icon, value, display, options, onChange, disabled }: {
  label: string; icon: ComponentType<{ className?: string }>; value: string; display: ReactNode
  options: { value: string; label: ReactNode; disabled?: boolean }[]; onChange: (value: string) => void; disabled?: boolean
}) {
  return <DropdownMenu>
    <DropdownMenuTrigger asChild>
      <Button variant="ghost" size="sm" disabled={disabled} aria-label={label} title={label} className="h-7 min-w-0 max-w-56 gap-1.5 px-2 font-normal">
        <Icon className="size-3.5 shrink-0 text-muted-foreground" />
        <span className="min-w-0 truncate">{display}</span>
        <ChevronDown className="size-3 shrink-0 text-muted-foreground" />
      </Button>
    </DropdownMenuTrigger>
    <DropdownMenuContent align="start" className="min-w-48">
      <DropdownMenuLabel>{label}</DropdownMenuLabel>
      <DropdownMenuRadioGroup value={value} onValueChange={onChange}>
        {options.map(option => <DropdownMenuRadioItem key={option.value} value={option.value} disabled={option.disabled}>{option.label}</DropdownMenuRadioItem>)}
      </DropdownMenuRadioGroup>
    </DropdownMenuContent>
  </DropdownMenu>
}

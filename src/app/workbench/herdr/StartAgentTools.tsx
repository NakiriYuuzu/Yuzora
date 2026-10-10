import { useId, useState } from "react"
import { useTranslation } from "react-i18next"
import { Play } from "lucide-react"
import { Button } from "@/components/ui/button"
import { Field, FieldDescription, FieldLabel } from "@/components/ui/field"
import { Textarea } from "@/components/ui/textarea"
import { ToggleGroup, ToggleGroupItem } from "@/components/ui/toggle-group"
import { AGENT_NAME_PATTERN, paneHasAgent, suggestAgentName } from "@/lib/herdrActions"
import type { HerdrSnapshot } from "@/lib/herdrTypes"
import { AgentLogo } from "../AgentLogo"
import { Advanced, PaneChoices, ReasonNote, TextField, ToolStep } from "./controls"
import type { HerdrOperation } from "./useHerdrOperation"

const commonAgentKinds = ["claude", "codex", "pi", "gemini", "cursor", "opencode"]
const moreAgentKinds = ["devin", "agy", "cline", "omp", "mastracode", "copilot", "kimi", "kiro", "droid", "amp", "grok", "hermes", "kilo", "qodercli", "qwen", "letta", "maki", "muse"]
const chip = "h-8 justify-start gap-2 border px-2 text-xs data-[state=on]:border-primary/40 data-[state=on]:bg-primary/5"

export function StartAgentTools({ snapshot, paneId, preferredPane, operation, can, reasonFor }: {
  snapshot: HerdrSnapshot | null; paneId: string; preferredPane?: string; operation: HerdrOperation
  can: (method: string) => boolean; reasonFor: (method: string) => string | null
}) {
  const { t } = useTranslation("herdrTools")
  // Prefer a pane that is not already running an agent unless the entry point named one.
  const freePane = snapshot?.terminals.find(pane => pane.paneId && !paneHasAgent(snapshot, pane.paneId))?.paneId
  const [target, setTarget] = useState(preferredPane && snapshot?.terminals.some(pane => pane.paneId === preferredPane) ? preferredPane : paneHasAgent(snapshot, paneId) && freePane ? freePane : paneId)
  const [kind, setKind] = useState("codex")
  const [more, setMore] = useState(false)
  const [custom, setCustom] = useState<string | null>(null)
  const [args, setArgs] = useState("")
  const argsId = useId()
  const suggestion = suggestAgentName(kind, snapshot)
  const name = custom || suggestion
  const nameError = AGENT_NAME_PATTERN.test(name) ? null : t("agentNameInvalid")
  const exists = snapshot?.terminals.some(pane => pane.paneId === target)
  const blocked = reasonFor("agent.start") ?? (!exists ? t("reason.noPane") : nameError)
  const kinds = more ? [...commonAgentKinds, ...moreAgentKinds] : commonAgentKinds
  return <div className="flex min-w-0 flex-col gap-6">
    <ToolStep index={1} title={t("startWhere")}>
      <div className="flex min-w-0 flex-col gap-2">
        <p className="text-xs text-muted-foreground">{t("startAgentHint")}</p>
        <PaneChoices snapshot={snapshot} value={target} onChange={setTarget} disabled={operation.busy} preferred={preferredPane} agentAware />
      </div>
    </ToolStep>
    <ToolStep index={2} title={t("startWhich")}>
      <div className="flex min-w-0 flex-col gap-2">
        <ToggleGroup type="single" value={kind} onValueChange={value => { if (value) setKind(value) }} aria-label={t("agentKind")} disabled={operation.busy} className="grid grid-cols-3 gap-1.5 sm:grid-cols-4 lg:grid-cols-6">
          {kinds.map(item => <ToggleGroupItem key={item} value={item} className={chip}>
            <AgentLogo kind={item} label={item} /><span className="min-w-0 truncate">{item}</span>
          </ToggleGroupItem>)}
        </ToggleGroup>
        <Button variant="link" size="sm" className="h-6 self-start px-0 text-xs" onClick={() => setMore(value => !value)}>{t(more ? "fewerKinds" : "moreKinds")}</Button>
      </div>
    </ToolStep>
    <Advanced title={t("advanced")}>
      <TextField label={t("agentName")} hint={t("agentNameHint")} value={custom ?? suggestion} onChange={setCustom} placeholder={suggestion} error={custom ? nameError : null} disabled={operation.busy} />
      <Field><FieldLabel htmlFor={argsId}>{t("arguments")}</FieldLabel><Textarea id={argsId} value={args} onChange={event => setArgs(event.target.value)} rows={2} disabled={operation.busy} /><FieldDescription>{t("argumentsHint")}</FieldDescription></Field>
    </Advanced>
    <div className="flex min-w-0 flex-wrap items-center justify-end gap-3">
      <ReasonNote reason={blocked} className="min-w-0 flex-1" />
      <Button disabled={operation.busy || Boolean(blocked) || !can("agent.start")} title={blocked ?? undefined} onClick={() => void operation.run({ method: "agent.start", params: { pane_id: target, kind, name, args: args.split("\n").filter(Boolean), timeout_ms: 30000 } })}>
        <Play data-icon="inline-start" />{t("start")}
      </Button>
    </div>
  </div>
}

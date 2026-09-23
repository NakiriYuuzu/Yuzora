import { useId, useState } from "react"
import { useTranslation } from "react-i18next"
import { Button } from "@/components/ui/button"
import { Field, FieldDescription, FieldLabel } from "@/components/ui/field"
import { Textarea } from "@/components/ui/textarea"
import { ToggleGroup, ToggleGroupItem } from "@/components/ui/toggle-group"
import type { HerdrSnapshot } from "@/lib/herdrTypes"
import { AgentLogo } from "../AgentLogo"
import { PaneChoices, TextField, ToolStep } from "./controls"
import type { HerdrOperation } from "./useHerdrOperation"

const kinds = ["claude", "codex", "pi", "gemini", "cursor", "devin", "agy", "cline", "omp", "mastracode", "opencode", "copilot", "kimi", "kiro", "droid", "amp", "grok", "hermes", "kilo", "qodercli", "qwen", "letta", "maki", "muse"]
const chip = "h-7 border px-2.5 text-xs data-[state=on]:border-primary/40 data-[state=on]:bg-primary/5"
export function AgentTools({ snapshot, paneId, operation, can }: { snapshot: HerdrSnapshot | null; paneId: string; operation: HerdrOperation; can: (method: string) => boolean }) {
  const { t } = useTranslation("herdrTools")
  const [target, setTarget] = useState(paneId)
  const [kind, setKind] = useState("codex")
  const [name, setName] = useState("")
  const [args, setArgs] = useState("")
  const [prompt, setPrompt] = useState("")
  const [wait, setWait] = useState("settled")
  const promptId = useId()
  const argsId = useId()
  const exists = snapshot?.terminals.some(pane => pane.paneId === target)
  const disabled = (method: string) => operation.busy || !exists || !can(method)
  const until = wait === "settled" ? [] : [wait]
  return <div className="flex min-w-0 flex-col gap-6">
    <ToolStep index={1} title={t("pane")}>
      <PaneChoices snapshot={snapshot} value={target} onChange={setTarget} disabled={operation.busy} />
    </ToolStep>
    <ToolStep index={2} title={t("startAgent")}>
      <div className="flex min-w-0 flex-col gap-3">
        <p className="text-xs text-muted-foreground">{t("startAgentHint")}</p>
        <ToggleGroup type="single" value={kind} onValueChange={value => { if (value) setKind(value) }} aria-label={t("agentKind")} disabled={operation.busy} className="grid grid-cols-3 gap-1.5 sm:grid-cols-4 lg:grid-cols-6">
          {kinds.map(item => <ToggleGroupItem key={item} value={item} className={`${chip} h-8 justify-start gap-2 px-2`}>
            <AgentLogo kind={item} label={item} /><span className="min-w-0 truncate">{item}</span>
          </ToggleGroupItem>)}
        </ToggleGroup>
        <TextField label={t("name")} value={name} onChange={setName} pattern="[a-z][a-z0-9_-]{0,31}" placeholder="reviewer" />
        <Field><FieldLabel htmlFor={argsId}>{t("arguments")}</FieldLabel><Textarea id={argsId} value={args} onChange={event => setArgs(event.target.value)} rows={2} /><FieldDescription>{t("argumentsHint")}</FieldDescription></Field>
        <div className="flex flex-wrap justify-end gap-2">
          <Button variant="outline" disabled={disabled("agent.rename")} onClick={() => void operation.run({ method: "agent.rename", params: { target, name: name || undefined } })}>{t("rename")}</Button>
          <Button disabled={disabled("agent.start") || !/^[a-z][a-z0-9_-]{0,31}$/.test(name)} onClick={() => void operation.run({ method: "agent.start", params: { pane_id: target, kind, name, args: args.split("\n").filter(Boolean), timeout_ms: 30000 } })}>{t("start")}</Button>
        </div>
      </div>
    </ToolStep>
    <ToolStep index={3} title={t("promptAgent")}>
      <div className="flex min-w-0 flex-col gap-3">
        <p className="text-xs text-muted-foreground">{t("promptHint")}</p>
        <Field><FieldLabel htmlFor={promptId}>{t("prompt")}</FieldLabel><Textarea id={promptId} value={prompt} onChange={event => setPrompt(event.target.value)} rows={4} /></Field>
        <div className="flex min-w-0 flex-col gap-1.5">
          <span className="text-xs text-muted-foreground">{t("waitUntil")}</span>
          <ToggleGroup type="single" value={wait} onValueChange={value => { if (value) setWait(value) }} aria-label={t("waitUntil")} className="flex-wrap justify-start gap-1.5" disabled={operation.busy}>
            {["settled", "blocked", "idle", "done", "working"].map(item => <ToggleGroupItem value={item} key={item} className={chip}>{t(`state.${item}`)}</ToggleGroupItem>)}
          </ToggleGroup>
        </div>
        <div className="flex flex-wrap justify-end gap-2">
          <Button variant="outline" disabled={disabled("agent.explain")} onClick={() => void operation.run({ method: "agent.explain", params: { target } })}>{t("explain")}</Button>
          <Button variant="outline" disabled={disabled("agent.wait")} onClick={() => void operation.run({ method: "agent.wait", params: { target, until, timeout_ms: 120000 } })}>{t("wait")}</Button>
          <Button variant="outline" disabled={disabled("agent.prompt") || !prompt.trim()} onClick={() => void operation.run({ method: "agent.prompt", params: { target, text: prompt, wait: { until, timeout_ms: 120000 } } })}>{t("sendAndWait")}</Button>
          <Button disabled={disabled("agent.prompt") || !prompt.trim()} onClick={() => void operation.run({ method: "agent.prompt", params: { target, text: prompt } })}>{t("send")}</Button>
        </div>
        <div className="flex flex-wrap items-center gap-1.5">
          <span className="mr-1 text-xs text-muted-foreground">{t("sendKeys")}</span>
          {["esc", "ctrl+c", "enter", "up", "down"].map(key => <Button key={key} variant="outline" size="sm" className="h-7 font-mono text-xs" disabled={disabled("agent.send_keys")} onClick={() => void operation.run({ method: "agent.send_keys", params: { target, keys: [key] } })}>{key}</Button>)}
        </div>
      </div>
    </ToolStep>
  </div>
}

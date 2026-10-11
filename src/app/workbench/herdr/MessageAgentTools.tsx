import { useId, useState } from "react"
import { useTranslation } from "react-i18next"
import { Bot, Send } from "lucide-react"
import { Button } from "@/components/ui/button"
import { Empty, EmptyDescription, EmptyHeader, EmptyMedia, EmptyTitle } from "@/components/ui/empty"
import { Field, FieldLabel } from "@/components/ui/field"
import { Textarea } from "@/components/ui/textarea"
import { ToggleGroup, ToggleGroupItem } from "@/components/ui/toggle-group"
import { AGENT_NAME_PATTERN, agentPanes } from "@/lib/herdrActions"
import type { HerdrSnapshot } from "@/lib/herdrTypes"
import { cn } from "@/lib/utils"
import { Advanced, ChoiceCards, ReasonNote, TextField, ToolStep } from "./controls"
import type { HerdrOperation } from "./useHerdrOperation"

const chip = "h-7 border px-2.5 text-xs data-[state=on]:border-primary/40 data-[state=on]:bg-primary/5"

export function MessageAgentTools({ snapshot, paneId, preferredPane, operation, can, reasonFor, onStartAgent }: {
  snapshot: HerdrSnapshot | null; paneId: string; preferredPane?: string; operation: HerdrOperation
  can: (method: string) => boolean; reasonFor: (method: string) => string | null; onStartAgent: () => void
}) {
  const { t } = useTranslation("herdrTools")
  const agents = agentPanes(snapshot)
  const [target, setTarget] = useState(agents.some(agent => agent.paneId === preferredPane) ? preferredPane! : agents.some(agent => agent.paneId === paneId) ? paneId : agents[0]?.paneId ?? "")
  const [prompt, setPrompt] = useState("")
  const [wait, setWait] = useState("settled")
  const [name, setName] = useState("")
  const promptId = useId()
  if (!agents.length) return <Empty className="border">
    <EmptyHeader>
      <EmptyMedia variant="icon"><Bot /></EmptyMedia>
      <EmptyTitle>{t("noAgentsTitle")}</EmptyTitle>
      <EmptyDescription>{t("noAgentsHint")}</EmptyDescription>
    </EmptyHeader>
    <Button onClick={onStartAgent} disabled={operation.busy}>{t("tasks.startAgent.title")}</Button>
  </Empty>
  const exists = agents.some(agent => agent.paneId === target)
  const until = wait === "settled" ? [] : [wait]
  const blocked = (method: string) => reasonFor(method) ?? (!exists ? t("reason.noAgent") : null)
  const disabled = (method: string) => operation.busy || Boolean(blocked(method)) || !can(method)
  const sendBlocked = blocked("agent.prompt") ?? (!prompt.trim() ? t("promptRequired") : null)
  const nameError = name && !AGENT_NAME_PATTERN.test(name) ? t("agentNameInvalid") : null
  const tabLabel = (workspaceId: string, tabId?: string | null) => [snapshot?.spaces.find(space => space.id === workspaceId)?.label, snapshot?.tabs.find(tab => tab.id === tabId)?.label].filter(Boolean).join(" · ")
  return <div className="flex min-w-0 flex-col gap-6">
    <ToolStep index={1} title={t("messageTarget")}>
      <ChoiceCards label={t("messageTarget")} value={target} onChange={setTarget} disabled={operation.busy} columns={2} options={agents.map(agent => ({
        value: agent.paneId!,
        icon: Bot,
        title: <>{agent.title ?? agent.name}{agent.paneId === preferredPane && <span className="ml-1.5 rounded-full bg-primary/10 px-1.5 text-[10px] font-normal text-primary">{t("selectedPane")}</span>}</>,
        description: <>
          <span className="font-mono">{[agent.name, agent.displayAgent].filter(Boolean).join(" · ")}</span>
          <span className={cn("ml-1.5", agent.status === "blocked" && "text-amber-600 dark:text-amber-400")}>{t(`state.${agent.status}`)}</span>
          <span className="block truncate">{tabLabel(agent.workspaceId, agent.tabId)}</span>
        </>,
      }))} />
    </ToolStep>
    <ToolStep index={2} title={t("promptAgent")}>
      <div className="flex min-w-0 flex-col gap-3">
        <p className="text-xs text-muted-foreground">{t("promptHint")}</p>
        <Field><FieldLabel htmlFor={promptId}>{t("prompt")}</FieldLabel><Textarea id={promptId} value={prompt} onChange={event => setPrompt(event.target.value)} rows={4} disabled={operation.busy} /></Field>
        <div className="flex min-w-0 flex-wrap items-center justify-end gap-3">
          <ReasonNote reason={sendBlocked} className="min-w-0 flex-1" />
          <Button variant="outline" disabled={operation.busy || Boolean(sendBlocked) || !can("agent.prompt")} title={sendBlocked ?? undefined} onClick={() => void operation.run({ method: "agent.prompt", params: { target, text: prompt, wait: { until, timeout_ms: 120000 } } })}>{t("sendAndWait")}</Button>
          <Button disabled={operation.busy || Boolean(sendBlocked) || !can("agent.prompt")} title={sendBlocked ?? undefined} onClick={() => void operation.run({ method: "agent.prompt", params: { target, text: prompt } })}><Send data-icon="inline-start" />{t("send")}</Button>
        </div>
      </div>
    </ToolStep>
    <Advanced title={t("advanced")}>
      <div className="flex min-w-0 flex-col gap-1.5">
        <span className="text-xs text-muted-foreground">{t("waitUntil")}</span>
        <ToggleGroup type="single" value={wait} onValueChange={value => { if (value) setWait(value) }} aria-label={t("waitUntil")} className="flex-wrap justify-start gap-1.5" disabled={operation.busy}>
          {["settled", "blocked", "idle", "done", "working"].map(item => <ToggleGroupItem value={item} key={item} className={chip}>{t(`state.${item}`)}</ToggleGroupItem>)}
        </ToggleGroup>
        <div><Button variant="outline" size="sm" disabled={disabled("agent.wait")} title={blocked("agent.wait") ?? undefined} onClick={() => void operation.run({ method: "agent.wait", params: { target, until, timeout_ms: 120000 } })}>{t("wait")}</Button></div>
      </div>
      <div className="flex flex-wrap items-center gap-1.5">
        <span className="mr-1 text-xs text-muted-foreground">{t("sendKeys")}</span>
        {["esc", "ctrl+c", "enter", "up", "down"].map(key => <Button key={key} variant="outline" size="sm" className="h-7 font-mono text-xs" disabled={disabled("agent.send_keys")} title={blocked("agent.send_keys") ?? undefined} onClick={() => void operation.run({ method: "agent.send_keys", params: { target, keys: [key] } })}>{key}</Button>)}
      </div>
      <div className="flex min-w-0 flex-col gap-1.5">
        <p className="text-xs text-muted-foreground">{t("explainHint")}</p>
        <div><Button variant="outline" size="sm" disabled={disabled("agent.explain")} title={blocked("agent.explain") ?? undefined} onClick={() => void operation.run({ method: "agent.explain", params: { target } })}>{t("explain")}</Button></div>
      </div>
      <div className="flex min-w-0 flex-wrap items-end gap-2">
        <div className="min-w-48 flex-1"><TextField label={t("agentName")} hint={t("renameHint")} value={name} onChange={setName} error={nameError} disabled={operation.busy} /></div>
        <Button variant="outline" size="sm" disabled={disabled("agent.rename") || Boolean(nameError)} title={blocked("agent.rename") ?? undefined} onClick={() => void operation.run({ method: "agent.rename", params: { target, name: name || undefined } })}>{t("rename")}</Button>
      </div>
    </Advanced>
  </div>
}

import { useEffect, useRef, useState } from "react"
import { useTranslation } from "react-i18next"
import { ArrowLeft, ChevronRight, FolderKanban, Layers, Server } from "lucide-react"
import { Dialog, DialogContent, DialogDescription, DialogHeader, DialogTitle } from "@/components/ui/dialog"
import { ScrollArea } from "@/components/ui/scroll-area"
import { Alert, AlertDescription, AlertTitle } from "@/components/ui/alert"
import { Collapsible, CollapsibleContent, CollapsibleTrigger } from "@/components/ui/collapsible"
import { isHerdrStartupPending, useHerdrStore } from "@/state/herdrStore"
import { useHerdrNativeStore } from "@/state/herdrNativeStore"
import { Button } from "@/components/ui/button"
import { useHerdrToolsStore, type HerdrTask, type HerdrToolsSelection } from "@/state/herdrToolsStore"
import { useUiStore } from "@/state/uiStore"
import { parseRuntimeScope, sessionScope } from "@/lib/herdrProvider"
import { herdrActionAvailability } from "@/lib/herdrActions"
import { LOCAL_HOST_ID, runtimeKey } from "@/lib/runtimeIdentity"
import { useHostStore } from "@/state/hostStore"
import { ScopeMenu } from "./controls"
import { useHerdrOperation } from "./useHerdrOperation"
import { HerdrActionHome } from "./HerdrActionHome"
import { taskIcons } from "./taskIcons"
import { WorktreeTools } from "./WorktreeTools"
import { StartAgentTools } from "./StartAgentTools"
import { MessageAgentTools } from "./MessageAgentTools"
import { PaneTools } from "./PaneTools"
import { SessionTools } from "./SessionTools"
import { IntegrationTools } from "./IntegrationTools"
import { PluginTools } from "./PluginTools"

const FIRST_FIELD = "input:not([disabled]), textarea:not([disabled]), button[role=combobox]:not([disabled]), [role=radio][aria-checked=true]:not([disabled])"

export default function HerdrToolsDialog({ selection }: { selection: HerdrToolsSelection }) {
  const { t } = useTranslation("herdrTools")
  const sessions = useHerdrStore(s => s.sessions)
  const herdrStartup = useHerdrStore(s => s.herdrStartup)
  const hosts = useHostStore(s => s.hosts)
  const hostConfigs = useHostStore(s => s.configs)
  const [scope, setScope] = useState(selection.sessionName)
  const hostId = parseRuntimeScope(scope).hostId
  const hostSessions = sessions.filter(session => parseRuntimeScope(sessionScope(session)!).hostId === hostId)
  const [task, setTask] = useState<HerdrTask | undefined>(selection.task)
  // Escape steps back only when the user navigated here from the home grid; a direct entry just closes.
  const [fromHome, setFromHome] = useState(false)
  const [requestedWorkspace, setRequestedWorkspace] = useState(selection.workspaceId ?? "")
  const runtime = useHerdrStore(s => s.runtimesBySession[scope])
  const snapshot = runtime?.snapshot ?? null
  // Pane targets always belong to the selected Space; a launcher pane only picks the default Space.
  const preferredPane = scope === selection.sessionName ? selection.paneId : undefined
  const preferredSpace = preferredPane ? snapshot?.terminals.find(pane => pane.paneId === preferredPane)?.workspaceId : undefined
  const workspaceId = [requestedWorkspace, preferredSpace].find(id => id && snapshot?.spaces.some(space => space.id === id)) ?? snapshot?.focusedWorkspaceId ?? snapshot?.spaces[0]?.id ?? ""
  const spacePanes = snapshot?.terminals.filter(pane => pane.paneId && pane.workspaceId === workspaceId) ?? []
  const paneId = !snapshot ? preferredPane ?? ""
    : [preferredPane, snapshot.focusedPaneId].find(id => id && spacePanes.some(pane => pane.paneId === id)) ?? spacePanes[0]?.paneId ?? ""
  const operation = useHerdrOperation(scope)
  const starting = isHerdrStartupPending({ herdrStartup }, hostSessions.find(session => sessionScope(session) === scope))
  const can = (method: string) => herdrActionAvailability(runtime, method, starting).ok
  const reasonFor = (method: string) => { const state = herdrActionAvailability(runtime, method, starting); return state.ok ? null : state.reason }
  const messages = operation.result?.messages ?? (operation.result?.details as { messages?: string[] } | undefined)?.messages
  const diagnostic = operation.result?.type === "agent_explain" || operation.result?.type === "plugin_log_list"
  const resultAgent = operation.result?.agent
  const agentStarting = operation.result?.type === "agent_started" && resultAgent !== null && typeof resultAgent === "object" && "launch_pending" in resultAgent && resultAgent.launch_pending === true
  const resultText = agentStarting ? t("agentStartPending") : messages?.filter(Boolean).join("\n") ?? operation.result?.text ?? null
  const bodyRef = useRef<HTMLDivElement>(null)
  const focusEntry = () => {
    const root = bodyRef.current
    if (!root) return
    const target = task
      ? root.querySelector<HTMLElement>(FIRST_FIELD) ?? root.querySelector<HTMLElement>("[role=radio]:not([disabled])")
      : root.querySelector<HTMLElement>("[data-task-card]:not([disabled])")
    ;(target ?? root.querySelector<HTMLElement>("[data-back], [data-task-card], button:not([disabled])"))?.focus()
  }
  useEffect(focusEntry, [task])
  const go = (next: HerdrTask | undefined) => { setFromHome(next !== undefined); setTask(next) }
  const selectScope = (next: string) => { setScope(next); setRequestedWorkspace(""); void useHerdrStore.getState().bootstrap(next) }
  const hostOptions = [{ value: LOCAL_HOST_ID, label: t("local") }, ...Object.values(hostConfigs).map(host => ({ value: host.hostId, label: host.label, disabled: !hosts[host.hostId]?.connection }))]
  const currentSession = hostSessions.find(session => sessionScope(session) === scope)
  const sessionOptions = [
    ...hostSessions.map(session => ({ value: sessionScope(session)!, label: <>{session.name}{(isHerdrStartupPending({ herdrStartup }, session) || !session.running) && <span className="text-muted-foreground"> · {t(isHerdrStartupPending({ herdrStartup }, session) ? "workbench:herdrNav.connecting" : "stopped")}</span>}</> })),
    ...(currentSession ? [] : [{ value: scope, label: t("sessionNone") }]),
  ]
  const spaceOptions = snapshot?.spaces.map(space => ({ value: space.id, label: space.label })) ?? []
  const showSpace = task === "plugins"
  const separator = <ChevronRight aria-hidden="true" className="size-3.5 shrink-0 text-muted-foreground/60" />
  const TaskIcon = task ? taskIcons[task] : null
  return <Dialog open onOpenChange={open => { if (!open && !operation.busy) useHerdrToolsStore.getState().close() }}>
    <DialogContent className="flex h-[min(680px,calc(100dvh-2rem))] min-h-0 flex-col gap-0 overflow-hidden p-0 sm:max-w-[880px]" showCloseButton={!operation.busy} aria-busy={operation.busy}
      onOpenAutoFocus={event => { event.preventDefault(); focusEntry() }}
      onEscapeKeyDown={event => { if (operation.busy) event.preventDefault(); else if (task && fromHome) { event.preventDefault(); go(undefined) } }}>
      <DialogHeader className="shrink-0 gap-2 border-b px-5 pt-4 pb-3 pr-12">
        <div className="flex min-w-0 items-baseline gap-3">
          <DialogTitle>{t("title")}</DialogTitle>
          <DialogDescription className="min-w-0 truncate text-xs">{t("description")}</DialogDescription>
        </div>
        <div className="flex min-w-0 items-center gap-1.5">
          <span className="shrink-0 text-xs text-muted-foreground">{t("actingOn")}</span>
          <nav aria-label={t("scope")} className="flex min-w-0 flex-1 items-center gap-0.5">
            <ScopeMenu label={t("host")} icon={Server} value={hostId} disabled={operation.busy} options={hostOptions}
              display={hostOptions.find(option => option.value === hostId)?.label ?? hostId}
              onChange={value => {
                const first = sessions.find(session => parseRuntimeScope(sessionScope(session)!).hostId === value)
                selectScope(sessionScope(first) ?? (value === LOCAL_HOST_ID ? "default" : runtimeKey({ hostId: value, sessionName: "default" })))
              }} />
            {separator}
            <ScopeMenu label={t("session")} icon={Layers} value={scope} disabled={operation.busy || !hostSessions.length} options={sessionOptions}
              display={currentSession?.name ?? t("sessionNone")} onChange={selectScope} />
            {showSpace && <>
              {separator}
              <ScopeMenu label={t("workspace")} icon={FolderKanban} value={workspaceId} disabled={operation.busy || !spaceOptions.length} options={spaceOptions}
                display={spaceOptions.find(option => option.value === workspaceId)?.label ?? "—"} onChange={setRequestedWorkspace} />
            </>}
          </nav>
        </div>
      </DialogHeader>
      <ScrollArea className="min-h-0 min-w-0 flex-1" viewportClassName="[&>div]:!block" contentClassName="flex min-w-0 flex-col gap-4 px-6 py-5">
        <div ref={bodyRef} className="flex min-w-0 flex-col gap-4">
          {runtime?.errorMessage && <Alert><AlertTitle>{t("runtimeNotice")}</AlertTitle><AlertDescription>{runtime.errorMessage}</AlertDescription></Alert>}
          {!currentSession && <Alert><AlertTitle>{t("sessionNone")}</AlertTitle><AlertDescription>{t("noSessionHint")}</AlertDescription></Alert>}
          {!task && <HerdrActionHome runtime={runtime} starting={starting} busy={operation.busy} onTask={next => go(next)}
            onNative={() => useHerdrNativeStore.getState().open({ sessionName: scope, paneId: paneId || undefined })}
            onNotifications={() => { useHerdrToolsStore.getState().close(); useUiStore.getState().openSettings("herdr") }} />}
          {task && TaskIcon && <div className="flex min-w-0 flex-col gap-3">
            <div className="flex min-w-0 items-start gap-3">
              <Button variant="ghost" size="sm" data-back="" className="-ml-2 shrink-0" disabled={operation.busy} onClick={() => go(undefined)}><ArrowLeft data-icon="inline-start" />{t("backToActions")}</Button>
            </div>
            <div className="flex min-w-0 items-start gap-3">
              <span className="flex size-8 shrink-0 items-center justify-center rounded-lg bg-primary/10 text-primary"><TaskIcon className="size-4" aria-hidden="true" /></span>
              <div className="min-w-0"><h3 className="text-base font-medium">{t(`tasks.${task}.title`)}</h3><p className="text-xs text-muted-foreground">{t(`tasks.${task}.explain`)}</p></div>
            </div>
          </div>}
          {task === "worktree" && <WorktreeTools key={scope} sessionName={scope} workspaceId={workspaceId} spaces={snapshot?.spaces ?? []} preferredSpace={scope === selection.sessionName ? selection.workspaceId : undefined} onWorkspaceChange={setRequestedWorkspace} operation={operation} can={can} reasonFor={reasonFor} />}
          {task === "startAgent" && <StartAgentTools key={scope} snapshot={snapshot} paneId={paneId} preferredPane={preferredPane} operation={operation} can={can} reasonFor={reasonFor} />}
          {task === "messageAgent" && <MessageAgentTools key={scope} snapshot={snapshot} paneId={paneId} preferredPane={preferredPane} operation={operation} can={can} reasonFor={reasonFor} onStartAgent={() => go("startAgent")} />}
          {task === "movePane" && <PaneTools key={scope} snapshot={snapshot} paneId={paneId} workspaceId={workspaceId} preferredPane={preferredPane} operation={operation} can={can} reasonFor={reasonFor} />}
          {task === "sessions" && <SessionTools key={scope} sessionName={scope} operation={operation} onSelect={setScope} />}
          {task === "integrations" && <IntegrationTools key={scope} sessionName={scope} operation={operation} can={can} />}
          {task === "plugins" && <PluginTools key={scope} sessionName={scope} workspaceId={workspaceId} paneId={paneId} operation={operation} can={can} />}
        </div>
        {operation.busy && <p role="status" className="text-xs text-muted-foreground">{t("working")}</p>}
        {operation.error && <Alert variant="destructive"><AlertTitle>{t("operationFailed")}</AlertTitle><AlertDescription className="break-all">{operation.error}</AlertDescription></Alert>}
        {operation.refreshError && <Alert><AlertTitle>{t("refreshFailed")}</AlertTitle><AlertDescription className="break-all">{operation.refreshError}</AlertDescription></Alert>}
        {operation.result && <Alert><AlertTitle>{t(agentStarting ? "agentStartSubmitted" : "operationComplete")}</AlertTitle>
          {resultText && <AlertDescription className="whitespace-pre-wrap break-all">{resultText}</AlertDescription>}
          {diagnostic && <Collapsible className="col-start-2 mt-1"><CollapsibleTrigger className="text-xs font-medium underline-offset-2 hover:underline">{t("showDetails")}</CollapsibleTrigger>
            <CollapsibleContent><pre className="mt-2 max-h-64 overflow-auto rounded-md bg-muted p-2 font-mono text-[11px] whitespace-pre-wrap break-all">{JSON.stringify(operation.result, null, 2)}</pre></CollapsibleContent></Collapsible>}
        </Alert>}
      </ScrollArea>
    </DialogContent>
  </Dialog>
}

import { useRef, useState } from "react"
import { useTranslation } from "react-i18next"
import { Bell, Bot, ChevronRight, FolderKanban, GitBranch, Layers, Move, Plug, Puzzle, Server, SquareTerminal, type LucideIcon } from "lucide-react"
import { Dialog, DialogContent, DialogDescription, DialogHeader, DialogTitle } from "@/components/ui/dialog"
import { Tabs, TabsContent, TabsList, TabsTrigger } from "@/components/ui/tabs"
import { ScrollArea } from "@/components/ui/scroll-area"
import { Alert, AlertDescription, AlertTitle } from "@/components/ui/alert"
import { useHerdrStore } from "@/state/herdrStore"
import { useHerdrNativeStore } from "@/state/herdrNativeStore"
import { Button } from "@/components/ui/button"
import { useHerdrToolsStore, type HerdrTool, type HerdrToolsSelection } from "@/state/herdrToolsStore"
import { parseRuntimeScope, sessionScope } from "@/lib/herdrProvider"
import { LOCAL_HOST_ID, runtimeKey } from "@/lib/runtimeIdentity"
import { useHostStore } from "@/state/hostStore"
import { hasHerdrMethod } from "@/lib/herdrCapabilities"
import { ScopeMenu } from "./controls"
import { useHerdrOperation } from "./useHerdrOperation"
import { WorktreeTools } from "./WorktreeTools"
import { AgentTools } from "./AgentTools"
import { PaneTools } from "./PaneTools"
import { SessionTools } from "./SessionTools"
import { IntegrationTools } from "./IntegrationTools"
import { PluginTools } from "./PluginTools"
import { NotificationTools } from "./NotificationTools"

const tools: { id: HerdrTool; icon: LucideIcon }[] = [
  { id: "worktrees", icon: GitBranch }, { id: "agents", icon: Bot }, { id: "panes", icon: Move }, { id: "sessions", icon: Layers },
  { id: "integrations", icon: Plug }, { id: "plugins", icon: Puzzle }, { id: "notifications", icon: Bell },
]

export default function HerdrToolsDialog({ selection }: { selection: HerdrToolsSelection }) {
  const { t } = useTranslation("herdrTools")
  const sessions = useHerdrStore(s => s.sessions)
  const hosts = useHostStore(s => s.hosts)
  const hostConfigs = useHostStore(s => s.configs)
  const [scope, setScope] = useState(selection.sessionName)
  const hostId = parseRuntimeScope(scope).hostId
  const hostSessions = sessions.filter(session => parseRuntimeScope(sessionScope(session)!).hostId === hostId)
  const [tool, setTool] = useState<HerdrTool>(selection.tool)
  const [requestedWorkspace, setRequestedWorkspace] = useState(selection.workspaceId ?? "")
  const runtime = useHerdrStore(s => s.runtimesBySession[scope])
  const snapshot = runtime?.snapshot ?? null
  const workspaceId = snapshot?.spaces.find(space => space.id === requestedWorkspace)?.id ?? snapshot?.focusedWorkspaceId ?? snapshot?.spaces[0]?.id ?? ""
  const paneId = (scope === selection.sessionName ? selection.paneId : null) ?? snapshot?.focusedPaneId ?? snapshot?.terminals.find(pane => pane.workspaceId === workspaceId)?.paneId ?? ""
  const operation = useHerdrOperation(scope)
  const can = (method: string) => runtime?.connectionState === "ready" && Boolean(runtime.capabilities?.server.running) && hasHerdrMethod(runtime?.capabilities, method)
  const messages = operation.result?.messages ?? (operation.result?.details as { messages?: string[] } | undefined)?.messages
  const diagnostic = operation.result?.type === "agent_explain" || operation.result?.type === "plugin_log_list"
  const resultAgent = operation.result?.agent
  const agentStarting = operation.result?.type === "agent_started" && resultAgent !== null && typeof resultAgent === "object" && "launch_pending" in resultAgent && resultAgent.launch_pending === true
  const resultText = agentStarting ? t("agentStartPending") : messages?.filter(Boolean).join("\n") ?? operation.result?.text ?? (diagnostic ? JSON.stringify(operation.result, null, 2) : null)
  const navRef = useRef<HTMLDivElement>(null)
  const selectScope = (next: string) => { setScope(next); setRequestedWorkspace(""); void useHerdrStore.getState().bootstrap(next) }
  const hostOptions = [{ value: LOCAL_HOST_ID, label: t("local") }, ...Object.values(hostConfigs).map(host => ({ value: host.hostId, label: host.label, disabled: !hosts[host.hostId]?.connection }))]
  const currentSession = hostSessions.find(session => sessionScope(session) === scope)
  const sessionOptions = [
    ...hostSessions.map(session => ({ value: sessionScope(session)!, label: <>{session.name}{!session.running && <span className="text-muted-foreground"> · {t("stopped")}</span>}</> })),
    ...(currentSession ? [] : [{ value: scope, label: t("sessionNone") }]),
  ]
  const spaceOptions = snapshot?.spaces.map(space => ({ value: space.id, label: space.label })) ?? []
  const showSpace = tool === "worktrees" || tool === "plugins"
  const separator = <ChevronRight aria-hidden="true" className="size-3.5 shrink-0 text-muted-foreground/60" />
  return <Dialog open onOpenChange={open => { if (!open && !operation.busy) useHerdrToolsStore.getState().close() }}>
    <DialogContent className="flex h-[min(680px,calc(100dvh-2rem))] min-h-0 flex-col gap-0 overflow-hidden p-0 sm:max-w-[880px]" showCloseButton={!operation.busy} aria-busy={operation.busy}
      onOpenAutoFocus={event => { event.preventDefault(); navRef.current?.querySelector<HTMLElement>("[data-state=active]")?.focus() }}>
      <DialogHeader className="shrink-0 gap-2 border-b px-5 pt-4 pb-3 pr-12">
        <div className="flex min-w-0 items-baseline gap-3">
          <DialogTitle>{t("title")}</DialogTitle>
          <DialogDescription className="min-w-0 truncate text-xs">{t("description")}</DialogDescription>
        </div>
        <div className="flex min-w-0 items-center gap-2">
          <nav aria-label={t("scope")} className="-ml-2 flex min-w-0 flex-1 items-center gap-0.5">
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
          <Button variant="outline" size="sm" className="shrink-0" disabled={operation.busy || !runtime?.capabilities?.api.snapshot || runtime.capabilities.server.compatible !== true} onClick={() => useHerdrNativeStore.getState().open({ sessionName: scope, paneId: paneId || undefined })}>
            <SquareTerminal data-icon="inline-start" />{t("openNative")}
          </Button>
        </div>
      </DialogHeader>
      <Tabs orientation="vertical" value={tool} onValueChange={value => setTool(value as HerdrTool)} className="min-h-0 flex-1 flex-row gap-0">
        <TabsList ref={navRef} variant="line" aria-label={t("toolsNav")} className="w-44 shrink-0 items-stretch justify-start gap-0.5 self-stretch group-data-vertical/tabs:h-full rounded-none border-r bg-muted/30 p-2">
          {tools.map(({ id, icon: Icon }) => <TabsTrigger key={id} value={id} disabled={operation.busy} className="h-8 flex-none gap-2 px-2.5 font-normal data-active:bg-background data-active:font-medium group-data-[variant=line]/tabs-list:data-active:bg-background group-data-vertical/tabs:after:hidden">
            <Icon className="size-4 text-muted-foreground" />{t(`tools.${id}`)}
          </TabsTrigger>)}
        </TabsList>
        <ScrollArea className="min-h-0 min-w-0 flex-1" viewportClassName="[&>div]:!block" contentClassName="flex min-w-0 flex-col gap-4 px-6 py-5">
          {runtime?.errorMessage && tool !== "notifications" && <Alert><AlertTitle>{t("runtimeNotice")}</AlertTitle><AlertDescription>{runtime.errorMessage}</AlertDescription></Alert>}
          <TabsContent value="worktrees"><WorktreeTools key={`${scope}:${workspaceId}`} sessionName={scope} workspaceId={workspaceId} operation={operation} can={can} /></TabsContent>
          <TabsContent value="agents"><AgentTools key={scope} snapshot={snapshot} paneId={paneId} operation={operation} can={can} /></TabsContent>
          <TabsContent value="panes"><PaneTools key={scope} snapshot={snapshot} paneId={paneId} workspaceId={workspaceId} operation={operation} can={can} /></TabsContent>
          <TabsContent value="sessions"><SessionTools key={scope} sessionName={scope} operation={operation} onSelect={setScope} /></TabsContent>
          <TabsContent value="integrations"><IntegrationTools key={scope} sessionName={scope} operation={operation} can={can} /></TabsContent>
          <TabsContent value="plugins"><PluginTools key={scope} sessionName={scope} workspaceId={workspaceId} paneId={paneId} operation={operation} can={can} /></TabsContent>
          <TabsContent value="notifications"><NotificationTools /></TabsContent>
          {operation.busy && <p role="status" className="text-xs text-muted-foreground">{t("working")}</p>}
          {operation.error && <Alert variant="destructive"><AlertTitle>{t("operationFailed")}</AlertTitle><AlertDescription className="break-all">{operation.error}</AlertDescription></Alert>}
          {operation.refreshError && <Alert><AlertTitle>{t("refreshFailed")}</AlertTitle><AlertDescription className="break-all">{operation.refreshError}</AlertDescription></Alert>}
          {operation.result && <Alert><AlertTitle>{t(agentStarting ? "agentStartSubmitted" : "operationComplete")}</AlertTitle>{resultText && <AlertDescription className="whitespace-pre-wrap break-all">{resultText}</AlertDescription>}</Alert>}
        </ScrollArea>
      </Tabs>
    </DialogContent>
  </Dialog>
}

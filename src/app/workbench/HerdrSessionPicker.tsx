import { useCallback, useEffect, useRef, useState, type RefObject } from "react";
import { useTranslation } from "react-i18next";
import { Laptop, RefreshCw, Server, SquareTerminal } from "lucide-react";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Command, CommandGroup, CommandItem, CommandList } from "@/components/ui/command";
import { Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle } from "@/components/ui/dialog";
import { Empty, EmptyContent, EmptyDescription, EmptyHeader, EmptyTitle } from "@/components/ui/empty";
import { Field, FieldGroup, FieldLabel, FieldSet } from "@/components/ui/field";
import { Label } from "@/components/ui/label";
import { RadioGroup, RadioGroupItem } from "@/components/ui/radio-group";
import { ScrollArea } from "@/components/ui/scroll-area";
import { Tabs, TabsContent, TabsList, TabsTrigger } from "@/components/ui/tabs";
import { parseRuntimeScope, sessionScope } from "@/lib/herdrProvider";
import type { HerdrNamedSession, HerdrRuntimeSelection } from "@/lib/herdrTypes";
import { wslDistributions, type HostTarget, type WslDistribution } from "@/lib/hostIpc";
import { isWindowsPlatform } from "@/lib/platform";
import { LOCAL_HOST_ID } from "@/lib/runtimeIdentity";
import { useHerdrStore } from "@/state/herdrStore";
import { useHerdrToolsStore } from "@/state/herdrToolsStore";
import { selectionForHost, useHostStore } from "@/state/hostStore";
import { useRuntimePreferencesStore } from "@/state/runtimePreferencesStore";
import { useSshStore } from "@/state/sshStore";
import { useUiStore } from "@/state/uiStore";
import { HostList } from "./HostList";
import { RuntimeSourceFields } from "./RuntimeSourceFields";
import { runtimeSessionLabel } from "./spaceTreeIdentity";

type SessionSource = "connected" | "ssh" | "wsl";

/**
 * Join an existing Herdr Session (design A): sources on the left, the Session
 * list on the right, and a footer that always says what Load will do or why it
 * is unavailable. Host setup and the SSH host book are reused unchanged.
 */
export function HerdrSessionPicker({ initialSession, onSelect, onClose, returnFocusRef }: {
  initialSession: string | null;
  onSelect: (sessionName: string) => void;
  onClose: () => void;
  returnFocusRef: RefObject<HTMLButtonElement | null>;
}) {
  const { t } = useTranslation("spaceTree");
  const { t: th } = useTranslation("hosts");
  const sessions = useHerdrStore((state) => state.sessions);
  const currentSession = useHerdrStore((state) => state.selectedSessionName);
  const hosts = useHostStore((state) => state.hosts);
  const hostConfigs = useHostStore((state) => state.configs);
  const sshHosts = useSshStore((state) => state.hosts);
  const sshSessions = useSshStore((state) => state.sessions);
  const activeHostId = useSshStore((state) => state.activeHostId);
  const wslEnabled = useRuntimePreferencesStore((state) => state.wslEnabled);
  const windows = isWindowsPlatform();
  const [source, setSource] = useState<SessionSource>("connected");
  const [requestedSession, setRequestedSession] = useState(initialSession ?? "");
  const [requestedDistro, setRequestedDistro] = useState("");
  const [distros, setDistros] = useState<WslDistribution[] | null>(null);
  const [wslError, setWslError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const inFlight = useRef(false);
  const mounted = useRef(true);
  useEffect(() => {
    mounted.current = true;
    return () => { mounted.current = false; };
  }, []);
  const run = useCallback(async (action: () => Promise<unknown>) => {
    if (inFlight.current) return;
    inFlight.current = true;
    setBusy(true);
    setError(null);
    try { await action(); }
    catch (cause) { if (mounted.current) setError(cause instanceof Error ? cause.message : String(cause)); }
    finally {
      inFlight.current = false;
      if (mounted.current) setBusy(false);
    }
  }, []);
  useEffect(() => { void run(() => useHerdrStore.getState().refreshSessions()); }, [run]);
  useEffect(() => {
    if (!windows || !wslEnabled || source !== "wsl") return;
    let active = true;
    void wslDistributions().then((rows) => {
      if (active) { setDistros(rows); setWslError(null); }
    }).catch((cause) => { if (active) setWslError(String(cause)); });
    return () => { active = false; };
  }, [windows, wslEnabled, source]);

  const distro = distros?.find((item) => item.hostId === requestedDistro && item.version === 2)
    ?? distros?.find((item) => item.version === 2);
  const ssh = activeHostId ? sshSessions[activeHostId] : undefined;
  const hostId = source === "ssh" ? activeHostId : source === "wsl" ? distro?.hostId : null;
  const hostLabel = source === "wsl" ? distro?.name : sshHosts.find((item) => item.id === hostId)?.name;
  const target: HostTarget | null = source === "wsl" && windows && wslEnabled && distro
    ? { kind: "wsl", distro: distro.name }
    : source === "ssh" && ssh?.status === "connected" && ssh.sessionId
      ? { kind: "ssh", sessionId: ssh.sessionId } : null;
  const host = hostId ? hosts[hostId] : undefined;
  const hostReady = source === "connected" || Boolean(target && host?.connection &&
    JSON.stringify(host.target) === JSON.stringify(target));
  const availableSessions = source === "connected" ? sessions : hostReady
    ? sessions.filter((session) => parseRuntimeScope(sessionScope(session)!).hostId === hostId) : [];
  const runningSessions = availableSessions.filter((session) => session.running);
  const targetSession = runningSessions.some((session) => sessionScope(session) === requestedSession)
    ? requestedSession : sessionScope(runningSessions[0]) ?? "";
  const runningCount = sessions.filter((session) => session.running).length;

  function manageHost() {
    onClose();
    useUiStore.getState().openSettings("herdr", { hostId: hostId ?? undefined });
  }
  function openSessionTools() {
    // Only a Session of the selected host: tools must not target another machine.
    const scope = sessionScope(availableSessions[0]);
    if (!scope) return;
    onClose();
    useHerdrToolsStore.getState().open({ tool: "sessions", sessionName: scope });
  }
  async function connectRuntime(selection: HerdrRuntimeSelection) {
    if (!hostId || !target) return;
    await run(async () => {
      await useHostStore.getState().setup(hostId, hostLabel ?? hostId, target, selection);
      if (mounted.current) await useHerdrStore.getState().refreshSessions();
    });
  }
  async function loadSession(scope = targetSession) {
    if (!hostReady || !scope) return;
    await run(async () => {
      if (!useHerdrStore.getState().sessions.some((session) => session.running && sessionScope(session) === scope))
        throw new Error(t("sessionUnavailable"));
      await useHerdrStore.getState().selectSession(scope);
      if (!mounted.current) return;
      const state = useHerdrStore.getState();
      const runtime = state.runtimesBySession[scope];
      if (state.selectedSessionName !== scope || runtime?.connectionState !== "ready" ||
          !runtime.snapshot || runtime.errorMessage || runtime.capabilities?.server.compatible === false)
        throw new Error(runtime?.errorMessage ?? t("openFailed"));
      onSelect(scope);
    });
  }

  const selected = runningSessions.find((session) => sessionScope(session) === targetSession);
  const hint = busy ? t("loading")
    : source === "ssh" && !target ? t("pickerHintConnect")
      : hostId && target && !hostReady ? t("pickerHintSetup")
        : !runningSessions.length ? t("pickerHintNothing")
          : selected ? t("pickerHintLoad", { session: runtimeSessionLabel(targetSession, selected) })
            : t("pickerHintSelect");
  const sessionList = (items: HerdrNamedSession[]) => hostReady && (
    runningSessions.length || busy ? (
      <SessionList
        sessions={items}
        value={targetSession}
        current={currentSession}
        disabled={busy}
        kindOf={(scope) => {
          const id = parseRuntimeScope(scope).hostId;
          return id === LOCAL_HOST_ID ? "local" : hostConfigs[id]?.kind ?? "ssh";
        }}
        onValueChange={setRequestedSession}
        onLoad={(scope) => void loadSession(scope)}
      />
    ) : (
      <Empty className="herdr-session-picker-empty">
        <EmptyHeader>
          <EmptyTitle role="status">{t("noRunningSessions")}</EmptyTitle>
          <EmptyDescription>{t("pickerEmptyHint")}</EmptyDescription>
        </EmptyHeader>
        <EmptyContent className="flex-row justify-center">
          {availableSessions.length > 0 && <Button variant="outline" size="sm" onClick={openSessionTools}>{t("openSessionTools")}</Button>}
        </EmptyContent>
      </Empty>
    )
  );

  return <Dialog open onOpenChange={(open) => { if (!open) onClose(); }}>
    <DialogContent className="herdr-session-picker flex max-h-[calc(100dvh-2rem)] min-h-0 flex-col gap-0 p-0 sm:max-w-[720px]" onCloseAutoFocus={(event) => {
      event.preventDefault();
      returnFocusRef.current?.focus();
    }}>
      <DialogHeader className="shrink-0 px-5 pt-5 pb-3 pr-12">
        <DialogTitle>{t("addSession")}</DialogTitle>
        <DialogDescription>{t("sessionHostHint")}</DialogDescription>
      </DialogHeader>
      <Tabs orientation="vertical" value={source} className="herdr-session-picker-body" onValueChange={(value) => {
        if (value === "connected" || value === "ssh" || value === "wsl") {
          setSource(value); setRequestedSession(""); setError(null);
        }
      }}>
        <div className="herdr-session-picker-rail">
          <p className="herdr-session-picker-rail-title" aria-hidden="true">{t("sessionSource")}</p>
          <TabsList variant="line" aria-label={t("sessionSource")} className="w-full items-stretch gap-0.5 p-0">
            <TabsTrigger value="connected" disabled={busy}>
              <Laptop data-icon="inline-start" />{t("connectedSessions")}<span className="herdr-session-picker-count" aria-hidden="true">{runningCount}</span>
            </TabsTrigger>
            <TabsTrigger value="ssh" disabled={busy}>
              <Server data-icon="inline-start" />{t("sshHosts")}<span className="herdr-session-picker-count" aria-hidden="true">{sshHosts.length}</span>
            </TabsTrigger>
            {windows && <TabsTrigger value="wsl" disabled={busy}>
              <SquareTerminal data-icon="inline-start" />WSL<span className="herdr-session-picker-count" aria-hidden="true">{wslEnabled ? distros?.length ?? "—" : t("pickerOff")}</span>
            </TabsTrigger>}
          </TabsList>
        </div>
        <ScrollArea className="min-h-0 min-w-0 flex-1" viewportClassName="[&>div]:!block" contentClassName="herdr-session-picker-main">
          <TabsContent value="connected" className="flex flex-col gap-3">
            <p className="herdr-session-picker-hint">{t("connectedSessionsHint")}</p>
            {sessionList(availableSessions)}
          </TabsContent>
          <TabsContent value="ssh" className="flex flex-col gap-3">
            <FieldSet disabled={busy}>
              <HostList />
              {ssh?.status === "connecting" && <p role="status">{th("connecting")}</p>}
              {ssh?.error && <p role="alert">{ssh.error}</p>}
              {!target && <p className="herdr-session-picker-hint">{t("connectSshHostHint")}</p>}
            </FieldSet>
          </TabsContent>
          {windows && <TabsContent value="wsl" className="flex flex-col gap-3">
            {wslEnabled ? <FieldGroup>
              <Field>
                <FieldLabel id="herdr-session-distro-label">{t("wslDistribution")}</FieldLabel>
                {distros && distros.length > 0 && <RadioGroup
                  aria-labelledby="herdr-session-distro-label"
                  value={distro?.hostId ?? ""}
                  disabled={busy}
                  onValueChange={(value) => { setRequestedDistro(value); setRequestedSession(""); setError(null); }}
                  className="herdr-session-picker-distros"
                >
                  {distros.map((item) => <Label key={item.hostId} className="herdr-session-picker-distro" data-disabled={item.version !== 2 || undefined}>
                    <RadioGroupItem value={item.hostId} disabled={item.version !== 2} aria-label={`${item.name} · WSL${item.version}`} />
                    <span className="font-mono">{item.name}</span>
                    <Badge variant={item.version === 2 ? "secondary" : "outline"}>{item.version === 2 ? "WSL2" : t("pickerWsl1Unsupported")}</Badge>
                  </Label>)}
                </RadioGroup>}
              </Field>
              {!distros && !wslError && <p role="status">{t("loading")}</p>}
              {distros?.length === 0 && <p role="status">{th("wslEmpty")}</p>}
              {wslError && <p role="alert">{wslError}</p>}
            </FieldGroup> : <FieldGroup><p>{th("wslDisabled")}</p><Button variant="outline" onClick={manageHost}>{t("runtimeSettings")}</Button></FieldGroup>}
          </TabsContent>}
          {source !== "connected" && hostId && target && !hostReady && <div className="herdr-session-picker-setup">
            <SessionHostSetup key={`${hostId}:${JSON.stringify(target)}`} hostId={hostId} busy={busy || !!host?.connecting} onConnect={connectRuntime} />
          </div>}
          {source !== "connected" && hostId && target && <Button variant="ghost" size="sm" className="self-start" disabled={busy} onClick={manageHost}>{th("manageHost")}</Button>}
          {source !== "connected" && hostReady && <div className="flex flex-col gap-2">
            <p className="herdr-session-picker-group-title">{t("pickerHostSessions", { host: hostLabel ?? hostId })}</p>
            {sessionList(availableSessions)}
          </div>}
          {(error || (source !== "connected" && host?.error)) && <p role="alert" className="herdr-session-picker-error [overflow-wrap:anywhere]">{error ?? host?.error}</p>}
        </ScrollArea>
      </Tabs>
      <DialogFooter className="herdr-session-picker-footer">
        <p className="herdr-session-picker-footer-hint" aria-live="polite">{hint}</p>
        <Button variant="ghost" disabled={busy} onClick={() => void run(() => useHerdrStore.getState().refreshSessions())}>
          <RefreshCw data-icon="inline-start" />{t("refreshSessions")}
        </Button>
        <Button disabled={busy || !hostReady || !targetSession} onClick={() => void loadSession()}>{t(busy ? "loading" : "loadSession")}</Button>
      </DialogFooter>
    </DialogContent>
  </Dialog>;
}

/**
 * Keyboard-first Session list on shadcn Command: ↑/↓ move, Enter or a
 * double-click loads, a single click only selects. Stopped Sessions stay
 * visible but disabled so the reason is clear.
 */
function SessionList({ sessions, value, current, disabled, kindOf, onValueChange, onLoad }: {
  sessions: HerdrNamedSession[];
  value: string;
  current: string | null;
  disabled: boolean;
  kindOf: (scope: string) => "local" | "ssh" | "wsl";
  onValueChange: (scope: string) => void;
  onLoad: (scope: string) => void;
}) {
  const { t } = useTranslation("spaceTree");
  const runtimes = useHerdrStore((state) => state.runtimesBySession);
  const groups = new Map<string, HerdrNamedSession[]>();
  for (const session of sessions) {
    const scope = sessionScope(session)!;
    const hostId = parseRuntimeScope(scope).hostId;
    groups.set(hostId, [...(groups.get(hostId) ?? []), session]);
  }
  const hostTitle = (scope: string, session: HerdrNamedSession) =>
    runtimeSessionLabel(scope, session).split(" · ").slice(0, -1).join(" · ") || session.name;
  return <Command
    value={value}
    onValueChange={(next) => { if (!disabled) onValueChange(next); }}
    disablePointerSelection
    shouldFilter={false}
    loop
    tabIndex={0}
    aria-label={t("targetSession")}
    className="herdr-session-picker-list"
    onKeyDown={(event) => {
      if (event.key !== "Enter") return;
      event.preventDefault();
      if (!disabled && value) onLoad(value);
    }}
  >
    <CommandList>
      {[...groups.values()].map((items) => {
        const first = sessionScope(items[0])!;
        return <CommandGroup key={first} heading={hostTitle(first, items[0])}>
          {items.map((session) => {
            const scope = sessionScope(session)!;
            const kind = kindOf(scope);
            const Icon = kind === "local" ? Laptop : kind === "wsl" ? SquareTerminal : Server;
            return <CommandItem
              key={scope}
              value={scope}
              disabled={disabled || !session.running}
              aria-label={`${runtimeSessionLabel(scope, session)}${session.running ? "" : ` · ${t("sessionNotRunning")}`}`}
              className="herdr-session-picker-row"
              onSelect={() => onValueChange(scope)}
              onDoubleClick={() => { if (!disabled && session.running) { onValueChange(scope); onLoad(scope); } }}
            >
              <span className="herdr-session-picker-glyph" aria-hidden="true"><Icon /></span>
              <span className="herdr-session-picker-row-main">
                <span className="herdr-session-picker-row-title">{session.name}</span>
                <span className="herdr-session-picker-row-sub">{!session.running
                  ? t("pickerStoppedHint")
                  : runtimes[scope]?.snapshot
                    ? t("pickerSessionSummary", { spaces: runtimes[scope]!.snapshot!.spaces.length, agents: runtimes[scope]!.snapshot!.agents.length })
                    : hostTitle(scope, session)}</span>
              </span>
              {scope === current
                ? <Badge variant="secondary">{t("pickerCurrent")}</Badge>
                : <Badge variant="outline" data-status={session.running ? "running" : "stopped"}>{t(session.running ? "pickerRunning" : "sessionNotRunning")}</Badge>}
            </CommandItem>;
          })}
        </CommandGroup>;
      })}
    </CommandList>
  </Command>;
}

function SessionHostSetup({ hostId, busy, onConnect }: {
  hostId: string;
  busy: boolean;
  onConnect: (selection: HerdrRuntimeSelection) => Promise<void>;
}) {
  const { t } = useTranslation("hosts");
  const [selection, setSelection] = useState(() => selectionForHost(useHostStore.getState().configs[hostId]));
  return <FieldGroup>
    <RuntimeSourceFields value={selection} onChange={setSelection} disabled={busy} />
    <p>{t("setupDescription")}</p>
    <Button disabled={busy || (selection.source === "custom" && !selection.customPath?.trim())} onClick={() => void onConnect(selection)}>
      {t(busy ? "settingUp" : "setupHost")}
    </Button>
  </FieldGroup>;
}

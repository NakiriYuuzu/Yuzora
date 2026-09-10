import { useCallback, useEffect, useRef, useState, type RefObject } from "react";
import { useTranslation } from "react-i18next";
import { RefreshCw } from "lucide-react";
import { Button } from "@/components/ui/button";
import { Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle } from "@/components/ui/dialog";
import { Field, FieldGroup, FieldLabel, FieldSet } from "@/components/ui/field";
import { ScrollArea } from "@/components/ui/scroll-area";
import { Select, SelectContent, SelectGroup, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select";
import { Tabs, TabsContent, TabsList, TabsTrigger } from "@/components/ui/tabs";
import { parseRuntimeScope, sessionScope } from "@/lib/herdrProvider";
import type { HerdrRuntimeSelection } from "@/lib/herdrTypes";
import { wslDistributions, type HostTarget, type WslDistribution } from "@/lib/hostIpc";
import { isWindowsPlatform } from "@/lib/platform";
import { useHerdrStore } from "@/state/herdrStore";
import { selectionForHost, useHostStore } from "@/state/hostStore";
import { useRuntimePreferencesStore } from "@/state/runtimePreferencesStore";
import { useSshStore } from "@/state/sshStore";
import { useUiStore } from "@/state/uiStore";
import { HostList } from "./HostList";
import { RuntimeSourceFields } from "./RuntimeSourceFields";
import { runtimeSessionLabel } from "./spaceTreeIdentity";

type SessionSource = "connected" | "ssh" | "wsl";

export function HerdrSessionPicker({ initialSession, onSelect, onClose, returnFocusRef }: {
  initialSession: string | null;
  onSelect: (sessionName: string) => void;
  onClose: () => void;
  returnFocusRef: RefObject<HTMLButtonElement | null>;
}) {
  const { t } = useTranslation("spaceTree");
  const { t: th } = useTranslation("hosts");
  const sessions = useHerdrStore((state) => state.sessions);
  const hosts = useHostStore((state) => state.hosts);
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
  const hostLabel = source === "wsl" ? distro?.name : sshHosts.find((host) => host.id === hostId)?.name;
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

  function manageHost() {
    onClose();
    useUiStore.getState().openSettings("herdr", { hostId: hostId ?? undefined });
  }
  async function connectRuntime(selection: HerdrRuntimeSelection) {
    if (!hostId || !target) return;
    await run(async () => {
      await useHostStore.getState().setup(hostId, hostLabel ?? hostId, target, selection);
      if (mounted.current) await useHerdrStore.getState().refreshSessions();
    });
  }
  async function loadSession() {
    if (!hostReady || !targetSession) return;
    await run(async () => {
      if (!useHerdrStore.getState().sessions.some((session) => session.running && sessionScope(session) === targetSession))
        throw new Error(t("sessionUnavailable"));
      await useHerdrStore.getState().selectSession(targetSession);
      if (!mounted.current) return;
      const state = useHerdrStore.getState();
      const runtime = state.runtimesBySession[targetSession];
      if (state.selectedSessionName !== targetSession || runtime?.connectionState !== "ready" ||
          !runtime.snapshot || runtime.errorMessage || runtime.capabilities?.server.compatible === false)
        throw new Error(runtime?.errorMessage ?? t("openFailed"));
      onSelect(targetSession);
    });
  }

  return <Dialog open onOpenChange={(open) => { if (!open) onClose(); }}>
    <DialogContent className="flex max-h-[calc(100dvh-2rem)] min-h-0 flex-col sm:max-w-[640px]" onCloseAutoFocus={(event) => {
      event.preventDefault();
      returnFocusRef.current?.focus();
    }}>
      <DialogHeader className="shrink-0 pr-6">
        <DialogTitle>{t("addSession")}</DialogTitle>
        <DialogDescription>{t("sessionHostHint")}</DialogDescription>
      </DialogHeader>
      <ScrollArea className="min-h-0 min-w-0 flex-1" viewportClassName="[&>div]:!block" contentClassName="flex min-w-0 flex-col gap-4 p-1">
        <Tabs value={source} onValueChange={(value) => {
          if (value === "connected" || value === "ssh" || value === "wsl") {
            setSource(value); setRequestedSession(""); setError(null);
          }
        }}>
          <TabsList aria-label={t("sessionSource")}>
            <TabsTrigger value="connected" disabled={busy}>{t("connectedSessions")}</TabsTrigger>
            <TabsTrigger value="ssh" disabled={busy}>{t("sshHosts")}</TabsTrigger>
            {windows && <TabsTrigger value="wsl" disabled={busy}>WSL</TabsTrigger>}
          </TabsList>
          <TabsContent value="connected"><p>{t("connectedSessionsHint")}</p></TabsContent>
          <TabsContent value="ssh">
            <FieldSet disabled={busy}>
              <HostList />
              {ssh?.status === "connecting" && <p role="status">{th("connecting")}</p>}
              {ssh?.error && <p role="alert">{ssh.error}</p>}
              {!target && <p>{t("connectSshHostHint")}</p>}
            </FieldSet>
          </TabsContent>
          {windows && <TabsContent value="wsl">
            {wslEnabled ? <FieldGroup>
              <Field>
                <FieldLabel htmlFor="herdr-session-distro">{t("wslDistribution")}</FieldLabel>
                <Select value={distro?.hostId ?? ""} onValueChange={(value) => { setRequestedDistro(value); setRequestedSession(""); setError(null); }} disabled={busy || !distros?.length}>
                  <SelectTrigger id="herdr-session-distro" className="w-full"><SelectValue placeholder={t("wslDistribution")} /></SelectTrigger>
                  <SelectContent><SelectGroup>{distros?.map((item) => <SelectItem key={item.hostId} value={item.hostId} disabled={item.version !== 2}>{item.name} · WSL{item.version}</SelectItem>)}</SelectGroup></SelectContent>
                </Select>
              </Field>
              {!distros && !wslError && <p role="status">{t("loading")}</p>}
              {distros?.length === 0 && <p role="status">{th("wslEmpty")}</p>}
              {wslError && <p role="alert">{wslError}</p>}
            </FieldGroup> : <FieldGroup><p>{th("wslDisabled")}</p><Button variant="outline" onClick={manageHost}>{t("runtimeSettings")}</Button></FieldGroup>}
          </TabsContent>}
        </Tabs>
        {hostId && target && !hostReady && <SessionHostSetup key={`${hostId}:${JSON.stringify(target)}`} hostId={hostId} busy={busy || !!host?.connecting} onConnect={connectRuntime} />}
        {source !== "connected" && hostId && target && <Button variant="outline" disabled={busy} onClick={manageHost}>{th("manageHost")}</Button>}
        {hostReady && <FieldGroup>
          <Field>
            <FieldLabel htmlFor="herdr-existing-session">{t("targetSession")}</FieldLabel>
            <Select value={targetSession} onValueChange={setRequestedSession} disabled={busy || !runningSessions.length}>
              <SelectTrigger id="herdr-existing-session" className="w-full"><SelectValue placeholder={t("selectSession")} /></SelectTrigger>
              <SelectContent><SelectGroup>{availableSessions.map((session) => <SelectItem key={sessionScope(session)} value={sessionScope(session)!} disabled={!session.running}>
                {runtimeSessionLabel(sessionScope(session)!, session)}{!session.running && ` · ${t("sessionNotRunning")}`}
              </SelectItem>)}</SelectGroup></SelectContent>
            </Select>
          </Field>
          {!busy && !runningSessions.length && <p role="status">{t("noRunningSessions")}</p>}
        </FieldGroup>}
        {(error || (source !== "connected" && host?.error)) && <p role="alert" className="[overflow-wrap:anywhere]">{error ?? host?.error}</p>}
      </ScrollArea>
      <DialogFooter className="shrink-0">
        <Button variant="outline" disabled={busy} onClick={() => void run(() => useHerdrStore.getState().refreshSessions())}>
          <RefreshCw data-icon="inline-start" />{t("refreshSessions")}
        </Button>
        <Button disabled={busy || !hostReady || !targetSession} onClick={() => void loadSession()}>{t(busy ? "loading" : "loadSession")}</Button>
      </DialogFooter>
    </DialogContent>
  </Dialog>;
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

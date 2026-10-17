import { useCallback, useEffect, useState } from "react"
import { useTranslation } from "react-i18next"
import { Alert, AlertDescription, AlertTitle } from "@/components/ui/alert"
import {
  AlertDialog,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
} from "@/components/ui/alert-dialog"
import { Badge } from "@/components/ui/badge"
import { Button } from "@/components/ui/button"
import { Card, CardContent, CardDescription, CardFooter, CardHeader, CardTitle } from "@/components/ui/card"
import { Checkbox } from "@/components/ui/checkbox"
import { Field, FieldDescription, FieldLabel } from "@/components/ui/field"
import { Input } from "@/components/ui/input"
import { Select, SelectContent, SelectGroup, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select"
import { Switch } from "@/components/ui/switch"
import {
  companionDeviceRevoke,
  companionDeviceUnlock,
  companionDeviceUpdate,
  companionDisable,
  companionEnable,
  companionExperimentalSet,
  companionSessionsSet,
  companionTailscaleProbe,
  companionTailscaleRemove,
  type CompanionDevice,
  type CompanionOverview,
  type CompanionWindow,
  type TailscaleProbe,
} from "@/lib/companionIpc"
import { describeCompanionError } from "@/lib/companionErrors"
import { useNow } from "@/lib/useNow"
import { useCompanionStore } from "@/state/companionStore"
import { useHerdrStore } from "@/state/herdrStore"
import { CompanionPairingDialog } from "./CompanionPairingDialog"

const WINDOWS: CompanionWindow[] = ["1h", "8h", "untilExit"]
const CONTROL_GRANTS = [["15m", 15], ["1h", 60], ["8h", 480]] as const

const refresh = () => useCompanionStore.getState().refresh()
const idle = (overview: CompanionOverview) => overview.status.state === "disabled" || overview.status.state === "failed"

/** RFC 3339 UTC without fractions, as the gateway stores grant deadlines. */
function untilIn(minutes: number): string {
  return new Date(Date.now() + minutes * 60_000).toISOString().replace(/\.\d{3}Z$/, "Z")
}

function useAction() {
  const { t } = useTranslation("companion")
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string | null>(null)
  /** Resolves true once the task succeeded; a failure is shown, not thrown. */
  const run = useCallback(async (task: () => Promise<unknown>) => {
    setBusy(true)
    setError(null)
    try { await task(); return true }
    catch (cause) { setError(String(cause)); return false }
    finally { setBusy(false) }
  }, [])
  const message = error && <p role="alert" className="text-[12px] text-destructive">{describeCompanionError(error, t)}</p>
  return { busy, run, message }
}

/**
 * Settings → Companion. The Experimental switch is always shown; everything
 * else only once it is on (plan 4.5/8.3).
 */
export function CompanionSettingsSection() {
  const { t } = useTranslation("companion")
  const overview = useCompanionStore(s => s.overview)
  const [loadError, setLoadError] = useState<string | null>(null)
  useEffect(() => { void refresh().catch(cause => setLoadError(String(cause))) }, [])
  if (!overview) {
    return loadError
      ? <p role="alert" className="text-[12px] text-destructive">{describeCompanionError(loadError, t)}</p>
      : <p className="text-[12px] text-muted-foreground">{t("loading")}</p>
  }
  return <div className="flex min-w-0 flex-col gap-4">
    {!overview.owner && <Alert><AlertTitle>{t("otherInstance.title")}</AlertTitle><AlertDescription>{t("otherInstance.description")}</AlertDescription></Alert>}
    <ExperimentalCard overview={overview} />
    {overview.settings.experimental && <>
      <Alert variant="destructive"><AlertTitle>{t("warning.title")}</AlertTitle><AlertDescription>{t("warning.description")}</AlertDescription></Alert>
      <TailscaleCard overview={overview} />
      <SessionsCard overview={overview} />
      <DevicesCard owner={overview.owner} />
    </>}
  </div>
}

function ExperimentalCard({ overview }: { overview: CompanionOverview }) {
  const { t } = useTranslation("companion")
  const { busy, run, message } = useAction()
  return <Card size="sm"><CardContent className="flex flex-col gap-2">
    <Field orientation="horizontal" data-settings-label={t("experimental.label")}>
      <div className="flex flex-1 flex-col gap-1">
        <FieldLabel htmlFor="companion-experimental">{t("experimental.label")}<Badge variant="secondary">{t("experimental.badge")}</Badge></FieldLabel>
        <FieldDescription>{t("experimental.description")}</FieldDescription>
      </div>
      <Switch
        id="companion-experimental"
        className="yz-switch"
        checked={overview.settings.experimental}
        disabled={busy || !overview.owner}
        onCheckedChange={enabled => void run(async () => useCompanionStore.getState().setOverview(await companionExperimentalSet(enabled)))}
      />
    </Field>
    {message}
  </CardContent></Card>
}

function TailscaleCard({ overview }: { overview: CompanionOverview }) {
  const { t } = useTranslation("companion")
  const { settings, status, owner } = overview
  const [probe, setProbe] = useState<TailscaleProbe | null>(null)
  const [probeError, setProbeError] = useState<string | null>(null)
  const [duration, setDuration] = useState<CompanionWindow>(settings.duration)
  const [confirming, setConfirming] = useState(false)
  const [pairing, setPairing] = useState(false)
  const closePairing = useCallback(() => setPairing(false), [])
  const { busy, run, message } = useAction()
  const now = useNow(30_000)

  const reprobe = useCallback(() => companionTailscaleProbe().then(
    next => { setProbe(next); setProbeError(null) },
    cause => setProbeError(String(cause)),
  ), [])
  useEffect(() => { void reprobe() }, [reprobe])
  const after = (task: () => Promise<unknown>) => run(async () => {
    try { await task() }
    finally { await Promise.all([refresh().catch(() => undefined), reprobe()]) }
  })

  const ready = probe?.cli && probe.node && probe.serve && !probe.refusal
  const stopped = idle(overview)
  const minutes = overview.enableUntil ? Math.max(0, Math.ceil((Date.parse(overview.enableUntil) - now) / 60_000)) : null
  const remaining = minutes === null ? null : minutes >= 60 ? `${Math.floor(minutes / 60)}h ${minutes % 60}m` : `${minutes}m`
  return <Card size="sm">
    <CardHeader>
      <CardTitle data-settings-label={t("tailscale.title")}>{t("tailscale.title")}</CardTitle>
      <CardDescription>{t("tailscale.description")}</CardDescription>
    </CardHeader>
    <CardContent className="flex flex-col gap-3">
      {probeError ? <p role="alert" className="text-[12px] text-destructive">{describeCompanionError(probeError, t)}</p>
        : !probe ? <p className="text-[12px] text-muted-foreground">{t("tailscale.checking")}</p>
          : !probe.cli ? <Alert><AlertTitle>{t("tailscale.noCli")}</AlertTitle><AlertDescription>{t("tailscale.install")}</AlertDescription></Alert>
            : <ProbeDetails probe={probe} />}
      {probe?.refusal && <Alert variant="destructive"><AlertDescription>{t(`refusal.${probe.refusal}`)}</AlertDescription></Alert>}
      {!!probe?.serve?.otherPorts.length && <Alert><AlertDescription>{t("tailscale.otherPorts", { ports: probe.serve.otherPorts.join(", ") })}</AlertDescription></Alert>}
      <p className="text-[12px] text-muted-foreground">{t("tailscale.certificateNote")}</p>
      <p className="text-[12px] text-muted-foreground">{t("tailscale.taggedNote")}</p>

      <div className="flex flex-wrap items-center gap-2 text-[12px]">
        <span className="text-muted-foreground">{t("status.label")}</span>
        <Badge data-testid="companion-status" variant={status.state === "failed" ? "destructive" : status.state === "running" ? "default" : "outline"}>{t(`status.${status.state}`)}</Badge>
        {status.state === "running" && <span>{settings.duration === "untilExit" ? t("status.untilExit") : remaining !== null && t("status.remaining", { time: remaining })}</span>}
      </div>
      {status.state === "failed" && <p role="alert" className="text-[12px] text-destructive">{describeCompanionError(status.code, t)}</p>}
      {stopped && settings.applied && <Alert>
        <AlertTitle>{t("leftover.title")}</AlertTitle>
        <AlertDescription className="flex flex-col items-start gap-2">
          <span>{t("leftover.description", { host: settings.applied.host, port: settings.applied.httpsPort })}</span>
          <Button size="sm" variant="outline" disabled={busy || !owner} onClick={() => void after(companionTailscaleRemove)}>{t("leftover.remove")}</Button>
        </AlertDescription>
      </Alert>}
      {message}
    </CardContent>
    <CardFooter className="flex flex-wrap items-center gap-2">
      {stopped ? <>
        <Select value={duration} onValueChange={value => setDuration(value as CompanionWindow)}>
          <SelectTrigger aria-label={t("enable.duration")} className="w-[160px]"><SelectValue /></SelectTrigger>
          <SelectContent><SelectGroup>{WINDOWS.map(value => <SelectItem key={value} value={value}>{t(`enable.window.${value}`)}</SelectItem>)}</SelectGroup></SelectContent>
        </Select>
        <Button size="sm" disabled={busy || !owner || !ready} onClick={() => setConfirming(true)}>{t("enable.button")}</Button>
      </> : <>
        <Button size="sm" variant="outline" disabled={busy || !owner} onClick={() => void after(companionDisable)}>{t("disable")}</Button>
        <Button size="sm" disabled={busy || !owner || status.state !== "running"} onClick={() => setPairing(true)}>{t("pairing.open")}</Button>
      </>}
      <Button size="sm" variant="ghost" disabled={busy} onClick={() => { setProbe(null); void reprobe() }}>{t("tailscale.recheck")}</Button>
    </CardFooter>
    <AlertDialog open={confirming} onOpenChange={setConfirming}>
      <AlertDialogContent className="sm:max-w-[480px]">
        <AlertDialogHeader>
          <AlertDialogTitle>{t("enable.confirmTitle")}</AlertDialogTitle>
          <AlertDialogDescription>{t("enable.confirmDescription")}</AlertDialogDescription>
        </AlertDialogHeader>
        <pre data-testid="companion-serve-command" className="whitespace-pre-wrap break-all rounded-md border bg-muted/40 p-2 font-mono text-[12px]">{probe?.command}</pre>
        <AlertDialogFooter>
          <AlertDialogCancel>{t("cancel")}</AlertDialogCancel>
          <Button variant="destructive" onClick={() => { setConfirming(false); void after(() => companionEnable(duration)) }}>{t("enable.confirm")}</Button>
        </AlertDialogFooter>
      </AlertDialogContent>
    </AlertDialog>
    {pairing && <CompanionPairingDialog onClose={closePairing} />}
  </Card>
}

function ProbeDetails({ probe }: { probe: TailscaleProbe }) {
  const { t } = useTranslation("companion")
  const { node, serve } = probe
  const yes = (value: boolean) => t(value ? "tailscale.yes" : "tailscale.no")
  return <dl className="grid grid-cols-[auto_minmax(0,1fr)] gap-x-3 gap-y-1 text-[12px]">
    <dt className="text-muted-foreground">{t("tailscale.cli")}</dt><dd className="break-all font-mono">{probe.cli?.path}</dd>
    {node && <>
      <dt className="text-muted-foreground">{t("tailscale.version")}</dt><dd>{node.version}</dd>
      <dt className="text-muted-foreground">{t("tailscale.state")}</dt><dd>{node.backendState}</dd>
      <dt className="text-muted-foreground">{t("tailscale.dnsName")}</dt><dd className="break-all font-mono">{node.dnsName ?? "—"}</dd>
      <dt className="text-muted-foreground">{t("tailscale.login")}</dt><dd className="break-all">{node.login ?? "—"}</dd>
      <dt className="text-muted-foreground">{t("tailscale.https")}</dt><dd>{yes(node.httpsReady)}</dd>
    </>}
    {serve && <>
      <dt className="text-muted-foreground">{t("tailscale.funnel")}</dt><dd>{yes(serve.funnel)}</dd>
      <dt className="text-muted-foreground">{t("tailscale.port", { port: probe.httpsPort })}</dt><dd>{t(`tailscale.portState.${serve.foreground ? "foreground" : serve.port}`)}</dd>
    </>}
  </dl>
}

function SessionsCard({ overview }: { overview: CompanionOverview }) {
  const { t } = useTranslation("companion")
  const { sessionAllowlist } = overview.settings
  const sessions = useHerdrStore(s => s.sessions)
  const local = sessions.filter(session => !session.hostId).map(session => session.name)
  const names = [...new Set([...local, ...sessionAllowlist])].sort()
  const [restart, setRestart] = useState(false)
  const { busy, run, message } = useAction()
  const toggle = (name: string, checked: boolean) => run(async () => {
    const next = checked ? [...sessionAllowlist, name] : sessionAllowlist.filter(item => item !== name)
    const result = await companionSessionsSet(next)
    setRestart(result.restartRequired)
    await refresh()
  })
  return <Card size="sm">
    <CardHeader>
      <CardTitle data-settings-label={t("sessions.title")}>{t("sessions.title")}</CardTitle>
      <CardDescription>{t("sessions.description")}</CardDescription>
    </CardHeader>
    <CardContent className="flex flex-col gap-2">
      {names.length === 0 && <p className="text-[12px] text-muted-foreground">{t("sessions.none")}</p>}
      {names.map(name => <label key={name} className="flex items-center gap-2 text-[13px]">
        <Checkbox checked={sessionAllowlist.includes(name)} disabled={busy || !overview.owner} onCheckedChange={checked => void toggle(name, checked === true)} />
        <span className="font-mono">{name}</span>
        {!local.includes(name) && <span className="text-[12px] text-muted-foreground">{t("sessions.notFound")}</span>}
      </label>)}
      {sessionAllowlist.length === 0 && names.length > 0 && <p className="text-[12px] text-muted-foreground">{t("sessions.emptyAllowlist")}</p>}
      {restart && <p role="status" className="text-[12px]">{t("sessions.restartRequired")}</p>}
      {message}
    </CardContent>
  </Card>
}

function DevicesCard({ owner }: { owner: boolean }) {
  const { t } = useTranslation("companion")
  const devices = useCompanionStore(s => s.devices)
  const sorted = [...devices].sort((a, b) => a.createdAt.localeCompare(b.createdAt))
  return <Card size="sm">
    <CardHeader>
      <CardTitle data-settings-label={t("devices.title")}>{t("devices.title")}</CardTitle>
      <CardDescription>{t("devices.description")}</CardDescription>
    </CardHeader>
    <CardContent className="flex flex-col gap-3">
      {sorted.length === 0 ? <p className="text-[12px] text-muted-foreground">{t("devices.none")}</p>
        : sorted.map(device => <DeviceRow key={device.deviceId} device={device} owner={owner} />)}
    </CardContent>
  </Card>
}

function DeviceRow({ device, owner }: { device: CompanionDevice; owner: boolean }) {
  const { t } = useTranslation("companion")
  const [draft, setDraft] = useState<string | null>(null)
  const label = draft ?? device.label
  const [revoking, setRevoking] = useState(false)
  const { busy, run, message } = useAction()
  const now = useNow(30_000)
  const until = device.grants.control ? Date.parse(device.grants.control.until) : 0
  const disabled = busy || !owner
  // A stopped gateway sends no device events: take the stored result.
  const store = (task: () => Promise<CompanionDevice>) => run(async () => useCompanionStore.getState().applyEvent({ type: "device.updated", device: await task() }))
  // One field per update: the other grant is copied back unchanged, so a
  // change is never both a tightening and a widening.
  const grant = (minutes: number | null) => store(() => companionDeviceUpdate(device.deviceId, {
    grants: { control: minutes === null ? null : { until: untilIn(minutes) }, full: device.grants.full },
  }))
  return <div data-testid="companion-device" className="flex flex-col gap-2 rounded-lg border p-3">
    <div className="flex flex-wrap items-center gap-2">
      <Input aria-label={t("devices.label")} className="h-7 w-[200px]" value={label} disabled={disabled} onChange={event => setDraft(event.target.value)} />
      {label !== device.label && <Button size="sm" variant="outline" disabled={disabled} onClick={() => void store(() => companionDeviceUpdate(device.deviceId, { label })).then(saved => { if (saved) setDraft(null) })}>{t("devices.saveLabel")}</Button>}
      {device.locked && <Badge variant="destructive">{t("devices.locked")}</Badge>}
      <span className="ml-auto text-[12px] text-muted-foreground">{t("devices.lastSeen", { time: new Date(device.lastSeen).toLocaleString() })}</span>
    </div>
    <div className="flex flex-wrap items-center gap-2 text-[12px]">
      <span>{until > now ? t("devices.controlUntil", { time: new Date(until).toLocaleString() }) : t("devices.controlNone")}</span>
      {CONTROL_GRANTS.map(([key, minutes]) => <Button key={key} size="sm" variant="outline" disabled={disabled} onClick={() => void grant(minutes)}>{t(`devices.grant.${key}`)}</Button>)}
      <Button size="sm" variant="outline" disabled={disabled || until <= now} onClick={() => void grant(null)}>{t("devices.grant.none")}</Button>
    </div>
    <div className="flex flex-wrap items-center gap-2">
      {device.locked && <Button size="sm" variant="outline" disabled={disabled} onClick={() => void store(() => companionDeviceUnlock(device.deviceId))}>{t("devices.unlock")}</Button>}
      <Button size="sm" variant="destructive" disabled={disabled} onClick={() => setRevoking(true)}>{t("devices.revoke")}</Button>
    </div>
    {message}
    <AlertDialog open={revoking} onOpenChange={setRevoking}>
      <AlertDialogContent className="sm:max-w-[420px]">
        <AlertDialogHeader>
          <AlertDialogTitle>{t("devices.revokeTitle", { label: device.label })}</AlertDialogTitle>
          <AlertDialogDescription>{t("devices.revokeDescription")}</AlertDialogDescription>
        </AlertDialogHeader>
        <AlertDialogFooter>
          <AlertDialogCancel>{t("cancel")}</AlertDialogCancel>
          <Button variant="destructive" onClick={() => { setRevoking(false); void run(async () => { await companionDeviceRevoke(device.deviceId); await refresh() }) }}>{t("devices.revoke")}</Button>
        </AlertDialogFooter>
      </AlertDialogContent>
    </AlertDialog>
  </div>
}

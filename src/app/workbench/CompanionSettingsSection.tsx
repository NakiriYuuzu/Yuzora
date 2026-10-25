import { useCallback, useEffect, useState } from "react"
import { useTranslation } from "react-i18next"
import { toast } from "sonner"
import { writeText } from "@tauri-apps/plugin-clipboard-manager"
import { openUrl } from "@tauri-apps/plugin-opener"
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
  companionNetCancel,
  companionNetLogin,
  companionNetLogout,
  companionSessionsSet,
  type CompanionDevice,
  type CompanionOverview,
  type CompanionWindow,
} from "@/lib/companionIpc"
import { describeCompanionError } from "@/lib/companionErrors"
import { companionAgo } from "@/lib/companionAgo"
import { companionRemaining } from "@/lib/companionRemaining"
import { useNow } from "@/lib/useNow"
import { useCompanionStore } from "@/state/companionStore"
import { useHerdrStore } from "@/state/herdrStore"
import { CompanionPairingDialog } from "./CompanionPairingDialog"
import { CompanionPhoneLinkDialog } from "./CompanionPhoneLinkDialog"

const WINDOWS: CompanionWindow[] = ["1h", "8h", "untilExit"]
const CONTROL_GRANTS = [["15m", 15], ["1h", 60], ["8h", 480]] as const
/** Where a tailnet admin turns on HTTPS Certificates. */
const TAILSCALE_DNS_URL = "https://login.tailscale.com/admin/dns"
/** Where a tailnet admin approves a device that waits for it. */
const TAILSCALE_MACHINES_URL = "https://login.tailscale.com/admin/machines"

const refresh = () => useCompanionStore.getState().refresh()
const idle = (overview: CompanionOverview) => overview.status.state === "disabled" || overview.status.state === "failed"

/** RFC 3339 UTC without fractions, as the gateway stores grant deadlines. */
function untilIn(minutes: number): string {
  return new Date(Date.now() + minutes * 60_000).toISOString().replace(/\.\d{3}Z$/, "Z")
}

/** A page of Tailscale's own, in the browser; failing to open it is a notice, not a card error. */
const openPage = (url: string) => void openUrl(url).catch(cause => toast.error(String(cause)))

function useAction() {
  const { t } = useTranslation("companion")
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const port = useCompanionStore(s => s.overview?.settings.gatewayPort)
  /** Resolves true once the task succeeded; a failure is shown, not thrown. */
  const run = useCallback(async (task: () => Promise<unknown>) => {
    setBusy(true)
    setError(null)
    try { await task(); return true }
    catch (cause) { setError(String(cause)); return false }
    finally { setBusy(false) }
  }, [])
  const message = error && <p role="alert" className="text-[12px] text-destructive">{describeCompanionError(error, t, port)}</p>
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
  const { settings, status, owner, net } = overview
  const [duration, setDuration] = useState<CompanionWindow>(settings.duration)
  const [confirming, setConfirming] = useState(false)
  const [signingOut, setSigningOut] = useState(false)
  const [switching, setSwitching] = useState(false)
  const [starting, setStarting] = useState(false)
  const [copied, setCopied] = useState(false)
  const [pairing, setPairing] = useState(false)
  const closePairing = useCallback(() => setPairing(false), [])
  const [linking, setLinking] = useState(false)
  const closeLinking = useCallback(() => setLinking(false), [])
  // A link made for a running Companion is of no use once it stops.
  if (linking && status.state !== "running") setLinking(false)
  const { busy, run, message } = useAction()
  const enableError = useCompanionStore(s => s.enableError)
  const now = useNow(30_000)

  const after = (task: () => Promise<unknown>) => run(async () => {
    useCompanionStore.getState().setEnableError(null)
    try { await task() }
    finally { await refresh().catch(() => undefined) }
  })
  const enable = async () => {
    const { setWelcomePending, setEnableError } = useCompanionStore.getState()
    setStarting(true)
    // The welcome host greets once the Companion serves, even if this card is gone by then.
    setWelcomePending(true)
    let failed = false
    // The failure goes to the store, not this card: the card may be gone when it comes, and the host tells the user then.
    await after(async () => {
      try { await companionEnable(duration) }
      catch (cause) { failed = true; setEnableError(String(cause)) }
    })
    setStarting(false)
    if (failed || useCompanionStore.getState().overview?.status.state !== "running") setWelcomePending(false)
  }

  const stopped = idle(overview)
  // Only an https page may be opened from what the node reports.
  const authUrl = net.authUrl?.startsWith("https://") ? net.authUrl : null
  // The node's mode tells what it runs for, also when this card was opened after the command started.
  const enabling = starting || (net.mode === "enable" && (status.state === "disabled" || status.state === "starting"))
  const loggingOut = net.mode === "logout"
  const signingIn = stopped && !enabling && net.mode === "login"
  const loginRequired = !!enableError?.startsWith("companion-net-login-required")
  const httpsDisabled = !!enableError?.startsWith("companion-net-https-disabled")
  const approvalRequired = !!enableError?.startsWith("companion-net-approval-required")
  const needsApproval = net.state === "needsApproval"
  // The machine an admin looks for: the name the node reports while it waits. Empty until it does: no name is guessed.
  const machine = net.dnsName?.split(".")[0] ?? ""
  const remaining = companionRemaining(overview.enableUntil, now)
  const progress = status.state === "starting" || net.cert?.state === "ready" ? "starting"
    : net.cert?.state === "error" ? "certificateRetry"
      : net.state === "running" ? "certificate" : "connecting"
  // A failure older than the sign-in or enable under way is not this run's.
  const failure = status.state === "failed" && !enabling && !signingIn ? describeCompanionError(status.code, t, settings.gatewayPort) : null
  const netFailure = net.error && stopped && !enabling ? describeCompanionError(net.error, t, settings.gatewayPort) : null
  const enableFailure = enableError ? describeCompanionError(enableError, t, settings.gatewayPort) : null
  const signIn = <Button size="sm" disabled={busy || !owner} onClick={() => void after(companionNetLogin)}>{t("tailscale.signIn")}</Button>
  return <Card size="sm">
    <CardHeader>
      <CardTitle data-settings-label={t("tailscale.title")}>{t("tailscale.title")}</CardTitle>
      <CardDescription>{t("tailscale.description")}</CardDescription>
    </CardHeader>
    <CardContent className="flex flex-col gap-3">
      {net.loggedIn && net.dnsName && <dl className="grid grid-cols-[auto_minmax(0,1fr)] gap-x-3 gap-y-1 text-[12px]">
        <dt className="text-muted-foreground">{t("tailscale.dnsName")}</dt><dd className="break-all font-mono">{net.dnsName}</dd>
        <dt className="text-muted-foreground">{t("tailscale.login")}</dt><dd className="break-all">{net.login ?? "—"}</dd>
      </dl>}
      {enabling ? <p role="status" className="text-[12px]">{t(`tailscale.progress.${progress}`)}</p>
        : loggingOut ? <p role="status" className="text-[12px]">{t("tailscale.progress.signingOut")}</p>
        : <div className="flex flex-wrap items-center gap-2 text-[12px]">
          <span className="text-muted-foreground">{t("status.label")}</span>
          <Badge data-testid="companion-status" variant={status.state === "failed" ? "destructive" : status.state === "running" ? "default" : "outline"}>{t(`status.${status.state}`)}</Badge>
          {status.state === "running" && <span>{settings.duration === "untilExit" ? t("status.untilExit") : remaining !== null && t("status.remaining", { time: remaining })}</span>}
        </div>}
      {stopped && !enabling && !loggingOut && (signingIn
        ? <p role="status" className="text-[12px]">{t(net.state === "needsLogin" ? "tailscale.progress.waitingForSignIn" : needsApproval ? (machine ? "tailscale.progress.waitingForApproval" : "tailscale.progress.waitingForApprovalUnnamed") : "tailscale.progress.connecting", { name: machine })}</p>
        : !net.loggedIn && <p className="text-[12px]">{t("tailscale.signedOut")}</p>)}
      {failure && <p role="alert" className="text-[12px] text-destructive">{failure}</p>}
      {netFailure && netFailure !== failure && <p role="alert" className="text-[12px] text-destructive">{netFailure}</p>}
      <p className="text-[12px] text-muted-foreground">{t("tailscale.certificateNote")}</p>
      <p className="text-[12px] text-muted-foreground">{t("tailscale.accessNote")}</p>
      {enableFailure && enableFailure !== failure && enableFailure !== netFailure && <p role="alert" className="text-[12px] text-destructive">{enableFailure}</p>}
      {message}
      {httpsDisabled && <div>
        <Button size="sm" variant="outline" disabled={busy} onClick={() => openPage(TAILSCALE_DNS_URL)}>{t("tailscale.openDnsSettings")}</Button>
      </div>}
      {approvalRequired && <div>
        <Button size="sm" variant="outline" disabled={busy} onClick={() => openPage(TAILSCALE_MACHINES_URL)}>{t("tailscale.openMachines")}</Button>
      </div>}
    </CardContent>
    {!enabling && !loggingOut && <CardFooter className="flex flex-wrap items-center gap-2">
      {!stopped ? <>
        <Button size="sm" variant="outline" disabled={busy || !owner} onClick={() => void after(companionDisable)}>{t("disable")}</Button>
        <Button size="sm" disabled={busy || !owner || status.state !== "running" || !net.serving} onClick={() => setPairing(true)}>{t("pairing.open")}</Button>
        <Button size="sm" variant="outline" disabled={busy || status.state !== "running" || !net.serving} onClick={() => setLinking(true)}>{t("phoneLink.open")}</Button>
      </> : signingIn ? <>
        {net.state === "needsLogin" && <>
          <Button size="sm" disabled={!authUrl} onClick={() => openPage(authUrl!)}>{t("tailscale.openBrowser")}</Button>
          <Button size="sm" variant="outline" disabled={busy || !authUrl} onClick={() => void run(async () => { await writeText(authUrl!); setCopied(true) })}>{t(copied ? "tailscale.copied" : "tailscale.copyLink")}</Button>
        </>}
        {needsApproval && <>
          <Button size="sm" onClick={() => openPage(TAILSCALE_MACHINES_URL)}>{t("tailscale.openMachines")}</Button>
          <Button size="sm" variant="outline" disabled={busy || !owner} onClick={() => setSwitching(true)}>{t("tailscale.useOtherAccount")}</Button>
        </>}
        <Button size="sm" variant="ghost" disabled={busy || !owner} onClick={() => void after(companionNetCancel)}>{t("cancel")}</Button>
      </> : !net.loggedIn ? signIn : <>
        <Select value={duration} onValueChange={value => setDuration(value as CompanionWindow)}>
          <SelectTrigger aria-label={t("enable.duration")} className="w-[160px]"><SelectValue /></SelectTrigger>
          <SelectContent><SelectGroup>{WINDOWS.map(value => <SelectItem key={value} value={value}>{t(`enable.window.${value}`)}</SelectItem>)}</SelectGroup></SelectContent>
        </Select>
        <Button size="sm" disabled={busy || !owner} onClick={() => setConfirming(true)}>{t("enable.button")}</Button>
        {loginRequired && signIn}
        <Button size="sm" variant="ghost" disabled={busy || !owner} onClick={() => setSigningOut(true)}>{t("tailscale.signOut")}</Button>
      </>}
    </CardFooter>}
    <AlertDialog open={confirming} onOpenChange={setConfirming}>
      <AlertDialogContent className="sm:max-w-[480px]">
        <AlertDialogHeader>
          <AlertDialogTitle>{t("enable.confirmTitle")}</AlertDialogTitle>
          <AlertDialogDescription>{t("enable.confirmDescription")}</AlertDialogDescription>
        </AlertDialogHeader>
        <AlertDialogFooter>
          <AlertDialogCancel>{t("cancel")}</AlertDialogCancel>
          <Button variant="destructive" onClick={() => { setConfirming(false); void enable() }}>{t("enable.confirm")}</Button>
        </AlertDialogFooter>
      </AlertDialogContent>
    </AlertDialog>
    <AlertDialog open={signingOut} onOpenChange={setSigningOut}>
      <AlertDialogContent className="sm:max-w-[480px]">
        <AlertDialogHeader>
          <AlertDialogTitle>{t("tailscale.signOutTitle")}</AlertDialogTitle>
          <AlertDialogDescription>{t("tailscale.signOutDescription")}</AlertDialogDescription>
        </AlertDialogHeader>
        <AlertDialogFooter>
          <AlertDialogCancel>{t("cancel")}</AlertDialogCancel>
          <Button variant="destructive" onClick={() => { setSigningOut(false); void after(companionNetLogout) }}>{t("tailscale.signOut")}</Button>
        </AlertDialogFooter>
      </AlertDialogContent>
    </AlertDialog>
    <AlertDialog open={switching} onOpenChange={setSwitching}>
      <AlertDialogContent className="sm:max-w-[480px]">
        <AlertDialogHeader>
          <AlertDialogTitle>{t("tailscale.useOtherAccountTitle")}</AlertDialogTitle>
          <AlertDialogDescription>{t("tailscale.useOtherAccountDescription")}</AlertDialogDescription>
        </AlertDialogHeader>
        <AlertDialogFooter>
          <AlertDialogCancel>{t("cancel")}</AlertDialogCancel>
          <Button variant="destructive" onClick={() => { setSwitching(false); void after(companionNetLogout) }}>{t("tailscale.useOtherAccount")}</Button>
        </AlertDialogFooter>
      </AlertDialogContent>
    </AlertDialog>
    {pairing && <CompanionPairingDialog onClose={closePairing} />}
    {linking && <CompanionPhoneLinkDialog onClose={closeLinking} />}
  </Card>
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
  const { t, i18n } = useTranslation("companion")
  const notifying = useCompanionStore(s => s.overview?.push.subscribed.includes(device.deviceId) ?? false)
  const lastGitRead = useCompanionStore(s => s.lastGitRead[device.deviceId])
  const [draft, setDraft] = useState<string | null>(null)
  const label = draft ?? device.label
  const [revoking, setRevoking] = useState(false)
  const { busy, run, message } = useAction()
  const now = useNow(30_000)
  const until = device.grants.control ? Date.parse(device.grants.control.until) : 0
  const gitReadAgo = lastGitRead ? companionAgo(lastGitRead, now, i18n.language) : null
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
      {notifying && <Badge variant="secondary">{t("devices.notifications")}</Badge>}
      <span className="ml-auto text-[12px] text-muted-foreground">{t("devices.lastSeen", { time: new Date(device.lastSeen).toLocaleString() })}</span>
    </div>
    {gitReadAgo && <p className="text-[12px] text-muted-foreground">{t("devices.lastGitRead", { time: gitReadAgo })}</p>}
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

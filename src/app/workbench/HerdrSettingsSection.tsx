import { useEffect, useRef, useState } from "react"
import { useTranslation } from "react-i18next"
import { open } from "@tauri-apps/plugin-dialog"
import { writeText } from "@tauri-apps/plugin-clipboard-manager"
import { Button } from "@/components/ui/button"
import { Card, CardContent, CardDescription, CardFooter, CardHeader, CardTitle } from "@/components/ui/card"
import { Alert, AlertDescription, AlertTitle } from "@/components/ui/alert"
import { Field, FieldDescription, FieldLabel } from "@/components/ui/field"
import { Select, SelectContent, SelectGroup, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select"
import { Switch } from "@/components/ui/switch"
import { herdrBinarySourceCheck, herdrBinarySourceGet, herdrBinarySourceSet } from "@/lib/herdrIpc"
import { checkHostRuntime, wslDistributions } from "@/lib/hostIpc"
import type { HostRuntimeCheck, HostTarget, WslDistribution } from "@/lib/hostIpc"
import type { HerdrBinarySourceInfo, HerdrRuntimeSelection, RuntimeBinaryCheck } from "@/lib/herdrTypes"
import { notifyHerdrBinarySourceChanged } from "@/lib/herdrBinarySourceEvents"
import { describeHerdrError } from "@/lib/herdrErrors"
import { sanitizeCustomPath, sanitizeSelection } from "@/lib/herdrPath"
import { isWindowsPlatform } from "@/lib/platform"
import { LOCAL_HOST_ID } from "@/lib/runtimeIdentity"
import { selectionForHost, useHostStore } from "@/state/hostStore"
import { useRuntimePreferencesStore } from "@/state/runtimePreferencesStore"
import { useRestartYuzora } from "@/state/useRestartYuzora"
import { useSshStore } from "@/state/sshStore"
import { useUiStore } from "@/state/uiStore"
import { DescribedError } from "./DescribedError"
import { RuntimeCheckView, RuntimeSourceFields } from "./RuntimeSourceFields"
import { NotificationTools } from "./herdr/NotificationTools"

export function HerdrSettingsSection() {
  const hostId = useUiStore(s => s.settingsHostId)
  const nonce = useUiStore(s => s.settingsNonce)
  const { t } = useTranslation("herdrTools")
  return <div className="flex min-w-0 flex-col gap-6">
    <RuntimeSettings key={`${nonce}:${hostId ?? LOCAL_HOST_ID}`} initialHostId={hostId ?? LOCAL_HOST_ID} />
    <Card size="sm"><CardHeader><CardTitle>{t("notificationSettings")}</CardTitle><CardDescription>{t("notificationSettingsHint")}</CardDescription></CardHeader><CardContent><NotificationTools /></CardContent></Card>
  </div>
}

function RuntimeSettings({ initialHostId }: { initialHostId: string }) {
  const { t } = useTranslation("runtimeSettings")
  const windows = isWindowsPlatform()
  const enabled = useRuntimePreferencesStore(s => s.wslEnabled)
  const configs = useHostStore(s => s.configs)
  const sshHosts = useSshStore(s => s.hosts)
  const sshSessions = useSshStore(s => s.sessions)
  const [hostId, setHostId] = useState(initialHostId)
  const [distros, setDistros] = useState<WslDistribution[]>([])
  const [error, setError] = useState<string | null>(null)
  useEffect(() => {
    if (!windows || !enabled) return
    let active = true
    void wslDistributions().then(rows => { if (active) setDistros(rows.filter(row => row.version === 2)) }).catch(error => { if (active) setError(String(error)) })
    return () => { active = false }
  }, [windows, enabled])
  const choices = new Map<string, { label: string; kind: "local" | "wsl" | "ssh"; distro?: string }>()
  choices.set(LOCAL_HOST_ID, { label: t(windows ? "windowsNative" : "local"), kind: "local" })
  for (const config of Object.values(configs)) choices.set(config.hostId, { label: config.label, kind: config.kind, distro: config.distro })
  for (const distro of distros) choices.set(distro.hostId, { label: distro.name, kind: "wsl", distro: distro.name })
  for (const host of sshHosts) choices.set(host.id, { label: host.name, kind: "ssh" })
  const selected = choices.get(hostId)
  const ssh = sshSessions[hostId]
  const target: HostTarget | null = selected?.kind === "wsl" && enabled && selected.distro
    ? { kind: "wsl", distro: selected.distro }
    : selected?.kind === "ssh" && ssh?.status === "connected" && ssh.sessionId ? { kind: "ssh", sessionId: ssh.sessionId } : null
  return <div className="flex min-w-0 flex-col gap-4">
    {windows && <Card size="sm"><CardHeader><CardTitle>{t("windowsRuntime")}</CardTitle><CardDescription>{t("windowsHint")}</CardDescription></CardHeader><CardContent>
      <Field orientation="horizontal"><div className="flex flex-1 flex-col gap-1"><FieldLabel htmlFor="wsl-runtime-enabled">{t("enableWsl")}</FieldLabel><FieldDescription>{t("enableWslHint")}</FieldDescription></div>
        <Switch id="wsl-runtime-enabled" className="yz-switch" checked={enabled} onCheckedChange={value => { void useRuntimePreferencesStore.getState().setWslEnabled(value).then(() => setError(null)).catch(error => setError(String(error))) }} />
      </Field>
    </CardContent></Card>}
    <Field><FieldLabel htmlFor="herdr-runtime-host">{t("host")}</FieldLabel><Select value={hostId} onValueChange={setHostId}>
      <SelectTrigger id="herdr-runtime-host" className="w-full"><SelectValue /></SelectTrigger>
      <SelectContent><SelectGroup>{[...choices].map(([id, choice]) => <SelectItem key={id} value={id}>{choice.label}{choice.kind !== "local" ? ` · ${choice.kind.toUpperCase()}` : ""}</SelectItem>)}</SelectGroup></SelectContent>
    </Select></Field>
    {error && <RuntimeError error={error} />}
    {selected?.kind === "local" ? <NativeRuntimeSettings />
      : selected?.kind === "wsl" && !enabled ? <Alert><AlertTitle>{t("wslDisabled")}</AlertTitle><AlertDescription>{t("enableWslHint")}</AlertDescription></Alert>
        : target && selected ? <RemoteRuntimeSettings key={`${hostId}:${JSON.stringify(target)}`} hostId={hostId} label={selected.label} target={target} />
          : selected?.kind === "ssh" ? <Card size="sm"><CardHeader><CardTitle>{selected.label}</CardTitle><CardDescription>{t("connectFirst")}</CardDescription></CardHeader><CardFooter><Button disabled={ssh?.status === "connecting"} onClick={() => useSshStore.getState().beginConnect(hostId)}>{t("connect")}</Button></CardFooter></Card>
            : <RuntimeError error={t("hostUnavailable")} />}
  </div>
}

function RuntimeError({ error }: { error: string }) {
  const { t } = useTranslation("runtimeSettings")
  return <DescribedError error={error} title={t("checkFailed")} />
}

function RestartControl() {
  const { t } = useTranslation("runtimeSettings")
  const { blocked, restart } = useRestartYuzora()
  const [failed, setFailed] = useState<string | null>(null)
  return <div className="flex flex-col gap-2">
    <div className="flex flex-wrap items-center gap-2">
      <Button size="sm" disabled={blocked} onClick={() => { setFailed(null); void restart().catch(error => setFailed(String(error))) }}>{t("restartNow")}</Button>
      {blocked && <p role="status" className="text-sm text-muted-foreground">{t("restartBlockedDirty")}</p>}
    </div>
    {failed && <RuntimeError error={failed} />}
  </div>
}

function NativeRuntimeSettings() {
  const { t } = useTranslation("runtimeSettings")
  const [info, setInfo] = useState<HerdrBinarySourceInfo | null>(null)
  const [selection, setSelection] = useState<HerdrRuntimeSelection>({ source: "default" })
  const [check, setCheck] = useState<RuntimeBinaryCheck | null>(null)
  const [busy, setBusy] = useState(false)
  const [loading, setLoading] = useState(true)
  const [error, setError] = useState<string | null>(null)
  const [notice, setNotice] = useState<string | null>(null)
  const [restartSaved, setRestartSaved] = useState(false)
  const { t: te } = useTranslation("herdrErrors")
  const { blocked: restartBlocked, restart } = useRestartYuzora()
  const generation = useRef(0)
  useEffect(() => {
    const counter = generation
    const token = ++counter.current
    void herdrBinarySourceGet().then(next => {
      if (counter.current !== token) return
      setInfo(next); setSelection({ source: next.configured, ...(next.configured === "custom" ? { customPath: next.customPath ?? next.configuredPath ?? "" } : {}) })
    }).catch(error => { if (counter.current === token) setError(String(error)) }).finally(() => { if (counter.current === token) setLoading(false) })
    return () => { counter.current++ }
  }, [])
  async function inspect() {
    const token = ++generation.current
    setBusy(true); setError(null); setNotice(null); setCheck(null)
    try { const clean = sanitizeSelection(selection); const next = await herdrBinarySourceCheck(clean.source, clean.customPath); if (generation.current === token) setCheck(next) }
    catch (error) { if (generation.current === token) setError(String(error)) }
    finally { if (generation.current === token) setBusy(false) }
  }
  async function apply() {
    const token = ++generation.current
    setBusy(true); setError(null)
    try {
      const clean = sanitizeSelection(selection)
      const result = await herdrBinarySourceSet(clean.source, clean.customPath)
      notifyHerdrBinarySourceChanged()
      const next = await herdrBinarySourceGet()
      if (generation.current === token) { setInfo(next); setRestartSaved(result.restartRequired); setNotice(t(result.restartRequired ? "savedRestart" : "saved")) }
    } catch (error) { if (generation.current === token) setError(String(error)) }
    finally { if (generation.current === token) setBusy(false) }
  }
  const customMissing = [info?.reason, info?.configuredReason, info?.configurationError].some(reason => reason && ["herdr-custom-path-not-executable", "herdr-custom-path-not-exe"].includes(describeHerdrError(reason, te).code ?? ""))
  async function revertToBundled() {
    const token = ++generation.current
    setBusy(true); setError(null); setNotice(null)
    try {
      await herdrBinarySourceSet("default")
      notifyHerdrBinarySourceChanged()
      const next = await herdrBinarySourceGet()
      if (generation.current !== token) return
      setInfo(next); setSelection({ source: "default" }); setCheck(null); setRestartSaved(true)
      if (restartBlocked) setNotice(t("revertSavedManualRestart"))
      else await restart()
    } catch (error) { if (generation.current === token) setError(String(error)) }
    finally { if (generation.current === token) setBusy(false) }
  }
  return <>
    <Card size="sm"><CardHeader><CardTitle>{t("current")}</CardTitle><CardDescription>{t("nativeCurrentHint")}</CardDescription></CardHeader><CardContent>
      {loading ? <p role="status">{t("loading")}</p> : <dl className="grid min-w-0 grid-cols-[auto_minmax(0,1fr)] gap-x-3 gap-y-2 text-sm">
        <dt>{t("source")}</dt><dd>{t(info?.active === "custom" ? "custom" : info?.active === "global" ? "installed" : "managed")}</dd>
        <dt>{t("binary")}</dt><dd className="break-all"><code>{info?.path ?? "—"}</code></dd>
        <dt>{t("client")}</dt><dd>{info?.version ?? "—"} · protocol {info?.protocol ?? "—"}</dd>
      </dl>}
      {info?.restartRequired && <Alert><AlertTitle>{t("savedRestart")}</AlertTitle><AlertDescription className="break-all">{info.configuredPath}</AlertDescription></Alert>}
      {info?.configurationError && <RuntimeError error={info.configurationError} />}
      {info?.reason && !info.available && <RuntimeError error={info.reason} />}
      {customMissing && <div className="flex flex-col gap-2"><p className="text-sm text-muted-foreground">{t("customMissingHint")}</p><div><Button variant="outline" size="sm" disabled={busy || loading} onClick={() => void revertToBundled()}>{t(restartBlocked ? "revertToBundled" : "revertToBundledRestart")}</Button></div></div>}
      {(info?.restartRequired || restartSaved) && <RestartControl />}
    </CardContent></Card>
    <Card size="sm"><CardHeader><CardTitle>{t("desired")}</CardTitle><CardDescription>{t("checkBeforeApply")}</CardDescription></CardHeader><CardContent className="flex flex-col gap-3">
      <RuntimeSourceFields local value={selection} disabled={busy || loading} onChange={value => { generation.current++; setSelection(value); setCheck(null); setNotice(null); setError(null) }} />
      {selection.source === "custom" && <Button variant="outline" disabled={busy || loading} onClick={() => { void open({ multiple: false, directory: false }).then(path => { if (typeof path === "string") { setSelection({ source: "custom", customPath: sanitizeCustomPath(path) }); setCheck(null) } }).catch(error => setError(String(error))) }}>{t("browseBinary")}</Button>}
      {check && <RuntimeCheckView check={check} />}
      {error && <RuntimeError error={error} />}
      {notice && <p role="status">{notice}</p>}
    </CardContent><CardFooter className="flex flex-wrap gap-2">
      <Button variant="outline" disabled={busy || loading} onClick={() => void inspect()}>{t(busy ? "checking" : "check")}</Button>
      <Button disabled={busy || !check?.canApply} onClick={() => void apply()}>{t("apply")}</Button>
      <Button variant="outline" disabled={busy} onClick={() => { void writeText(JSON.stringify({ host: "local", selection, current: info, target: check, error }, null, 2)).then(() => setNotice(t("copied"))).catch(error => setError(String(error))) }}>{t("copy")}</Button>
    </CardFooter></Card>
  </>
}

function RemoteRuntimeSettings({ hostId, label, target }: { hostId: string; label: string; target: HostTarget }) {
  const { t, i18n } = useTranslation("runtimeSettings")
  const config = useHostStore(s => s.configs[hostId])
  const hostError = useHostStore(s => s.hosts[hostId]?.error)
  const [selection, setSelection] = useState(() => selectionForHost(config))
  const [current, setCurrent] = useState<HostRuntimeCheck | null>(null)
  const [desired, setDesired] = useState<HostRuntimeCheck | null>(null)
  const [currentError, setCurrentError] = useState<string | null>(null)
  const [error, setError] = useState<string | null>(null)
  const [notice, setNotice] = useState<string | null>(null)
  const [busy, setBusy] = useState(false)
  const generation = useRef(0)
  useEffect(() => () => { generation.current++ }, [])
  async function inspect() {
    const token = ++generation.current
    setBusy(true); setError(null); setNotice(null); setDesired(null); setCurrent(null); setCurrentError(null)
    const results = await Promise.allSettled([
      checkHostRuntime(hostId, target, sanitizeSelection(selection)),
      config ? checkHostRuntime(hostId, target, { source: "custom", customPath: config.binary }) : Promise.resolve(null)
    ])
    if (generation.current !== token) return
    if (results[0].status === "fulfilled") setDesired(results[0].value); else setError(String(results[0].reason))
    if (results[1].status === "fulfilled") setCurrent(results[1].value); else setCurrentError(String(results[1].reason))
    setBusy(false)
  }
  async function apply() {
    const token = ++generation.current
    setBusy(true); setError(null)
    try {
      await useHostStore.getState().setup(hostId, label, target, sanitizeSelection(selection))
      if (generation.current !== token) return
      setNotice(t("saved")); setCurrent(null); setCurrentError(null); setDesired(null)
    } catch (error) { if (generation.current === token) setError(String(error)) }
    finally { if (generation.current === token) setBusy(false) }
  }
  return <>
    <Card size="sm"><CardHeader><CardTitle>{t("current")}</CardTitle><CardDescription>{label} · {t("savedPathHint")}</CardDescription></CardHeader><CardContent className="flex flex-col gap-3">
      <p className="break-all text-sm"><code>{config?.binary ?? t("notConfigured")}</code></p>
      {config?.helper && <p className="break-all text-sm">{t("helper")}: <code>{config.helper}</code></p>}
      {config?.verifiedAt && <p className="text-sm text-muted-foreground">{t("lastVerified")}: {new Date(config.verifiedAt).toLocaleString(i18n.language)}</p>}
      {current?.check && <RuntimeCheckView check={current.check} />}
      {currentError && <RuntimeError error={currentError} />}
      {hostError && <RuntimeError error={hostError} />}
    </CardContent></Card>
    <Card size="sm"><CardHeader><CardTitle>{t("desired")}</CardTitle><CardDescription>{t("remoteApplyHint")}</CardDescription></CardHeader><CardContent className="flex flex-col gap-3">
      <RuntimeSourceFields value={selection} disabled={busy} onChange={value => { generation.current++; setSelection(value); setDesired(null); setError(null); setNotice(null) }} />
      {desired && <>
        <p className="text-sm">{t("bundled")}: {desired.managedVersion} · protocol {desired.managedProtocol}</p>
        {selection.source === "default" && config && config.artifactIdentity !== desired.artifactIdentity && <Alert><AlertTitle>{t("updateAvailable")}</AlertTitle><AlertDescription>{t("updateAvailableHint")}</AlertDescription></Alert>}
        {desired.requiresInstall ? <Alert><AlertTitle>{t("needsInstall")}</AlertTitle><AlertDescription className="break-all">{desired.binary}</AlertDescription></Alert> : desired.check && <RuntimeCheckView check={desired.check} />}
      </>}
      {error && <RuntimeError error={error} />}
      {notice && <p role="status">{notice}</p>}
    </CardContent><CardFooter className="flex flex-wrap gap-2">
      <Button variant="outline" disabled={busy} onClick={() => void inspect()}>{t(busy ? "checking" : "check")}</Button>
      <Button disabled={busy || !desired || (!desired.requiresInstall && !desired.check?.canApply)} onClick={() => void apply()}>{t(selection.source === "default" ? "updateApply" : "apply")}</Button>
      <Button variant="outline" disabled={busy} onClick={() => { void writeText(JSON.stringify({ hostId, label, selection, saved: config, current, target: desired, currentError, error }, null, 2)).then(() => setNotice(t("copied"))).catch(error => setError(String(error))) }}>{t("copy")}</Button>
    </CardFooter></Card>
  </>
}

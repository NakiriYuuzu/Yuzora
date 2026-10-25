import { useId, useState } from "react"
import { useTranslation } from "react-i18next"
import { Eye, Smartphone } from "lucide-react"
import { Button } from "@/components/ui/button"
import { Popover, PopoverContent, PopoverTrigger } from "@/components/ui/popover"
import { companionDeviceUpdate, companionDisable, type CompanionDevice } from "@/lib/companionIpc"
import { describeCompanionError } from "@/lib/companionErrors"
import { companionRemaining } from "@/lib/companionRemaining"
import { useNow } from "@/lib/useNow"
import { useCompanionStore } from "@/state/companionStore"
import { useOverlayPresence } from "@/state/overlayStore"
import { useUiStore } from "@/state/uiStore"
import { paneLabel } from "@/workbench/companionAudit"

/** A control grant holds until its deadline; a full session has none, so it always holds. */
const holdsAccess = (device: CompanionDevice, at: number) =>
  device.grants.full || (device.grants.control !== null && Date.parse(device.grants.control.until) > at)

/**
 * Status bar chip for a running Companion: time left, how many phones are
 * connected, and a red "controlling" while one drives a pane. Hidden otherwise.
 */
export function CompanionStatusIndicator() {
  const running = useCompanionStore(s => s.overview?.status.state === "running")
  return running ? <Indicator /> : null
}

function Indicator() {
  const { t } = useTranslation("workbench")
  const { t: tCompanion } = useTranslation("companion")
  const enableUntil = useCompanionStore(s => s.overview?.enableUntil ?? null)
  const untilExit = useCompanionStore(s => s.overview?.settings.duration === "untilExit")
  const devices = useCompanionStore(s => s.devices)
  const peers = useCompanionStore(s => s.peers)
  const streams = useCompanionStore(s => s.streams)
  const [open, setOpen] = useState(false)
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const now = useNow(30_000)
  useOverlayPresence(open)

  const openStreams = Object.entries(streams)
  // Session events are best effort: a device with an open stream is connected even if its peer was never seen.
  const connected = [...new Set([...Object.values(peers), ...openStreams.map(([, stream]) => stream.deviceId)])]
  const controlling = openStreams.length > 0
  const remaining = untilExit ? t("statusBar.companion.untilExit") : companionRemaining(enableUntil, now)
  const canRevoke = devices.some(device => holdsAccess(device, now))
  const hintId = useId()
  const deviceLabel = (deviceId: string) => devices.find(device => device.deviceId === deviceId)?.label ?? t("statusBar.companion.unknownDevice")

  const act = async (task: () => Promise<unknown>) => {
    setBusy(true)
    setError(null)
    try { await task() }
    catch (cause) { setError(String(cause)) }
    finally { setBusy(false) }
  }
  // One device at a time: the gateway takes one admin command at once. Control and full both go
  // down in one update, a tightening that also signs the phone out: a full session cannot end otherwise.
  const revokeControl = () => act(async () => {
    for (const device of useCompanionStore.getState().devices) {
      if (!holdsAccess(device, Date.now())) continue
      const updated = await companionDeviceUpdate(device.deviceId, { grants: { control: null, full: false } })
      useCompanionStore.getState().applyEvent({ type: "device.updated", device: updated })
    }
  })
  const disable = () => act(async () => {
    try { await companionDisable() }
    finally { await useCompanionStore.getState().refresh().catch(() => undefined) }
  })

  return <Popover open={open} onOpenChange={setOpen}>
    <PopoverTrigger asChild>
      <button
        type="button"
        aria-label={t("statusBar.companion.ariaLabel")}
        data-testid="status-companion-chip"
        data-controlling={controlling}
        className="flex h-[22px] items-center gap-1 rounded-[6px] px-[6px] transition-colors duration-150 hover:bg-[rgba(var(--yz-accent-rgb),0.14)]"
      >
        <Smartphone className="size-[12px] shrink-0" aria-hidden="true" />
        {remaining && <span>{remaining}</span>}
        {connected.length > 0 && <span className="flex items-center gap-[3px]" title={t("statusBar.companion.viewers", { n: connected.length })}>
          <Eye className="size-[11px]" aria-hidden="true" />{connected.length}
        </span>}
        {controlling && <span className="font-medium" style={{ color: "var(--status-d)" }}>{t("statusBar.companion.controlling")}</span>}
      </button>
    </PopoverTrigger>
    <PopoverContent side="top" align="end" sideOffset={8} collisionPadding={12} aria-label={t("statusBar.companion.title")} className="w-80 font-sans">
      <h2 className="text-[13px] font-semibold">{t("statusBar.companion.title")}</h2>
      {connected.length === 0 ? <p className="text-[12px] text-muted-foreground">{t("statusBar.companion.empty")}</p>
        : <ul className="flex flex-col gap-1.5">
          {connected.map(deviceId => {
            const driven = openStreams.filter(([, stream]) => stream.deviceId === deviceId)
            return <li key={deviceId} data-testid="status-companion-device" className="flex flex-col text-[12px]">
              <span className="font-medium">{deviceLabel(deviceId)}</span>
              {driven.length === 0 ? <span className="text-muted-foreground">{t("statusBar.companion.deviceViewing")}</span>
                : driven.map(([key, stream]) => <span key={key} className="text-destructive">
                  {stream.scope === "full"
                    ? t("statusBar.companion.deviceFull", { session: stream.session })
                    : t("statusBar.companion.deviceControlling", { pane: paneLabel(stream.session, stream.target ?? null) })}
                </span>)}
            </li>
          })}
        </ul>}
      {error && <p role="alert" className="text-[12px] text-destructive">{describeCompanionError(error, tCompanion)}</p>}
      {canRevoke && <p id={hintId} className="text-[12px] text-muted-foreground">{t("statusBar.companion.revokeControlHint")}</p>}
      <div className="flex flex-wrap gap-2">
        <Button size="sm" variant="outline" disabled={busy || !canRevoke} aria-describedby={canRevoke ? hintId : undefined} onClick={() => void revokeControl()}>{t("statusBar.companion.revokeControl")}</Button>
        <Button size="sm" variant="outline" onClick={() => { setOpen(false); useUiStore.getState().openSettings("companion") }}>{t("statusBar.companion.settings")}</Button>
        <Button size="sm" variant="destructive" disabled={busy} onClick={() => void disable()}>{t("statusBar.companion.disable")}</Button>
      </div>
    </PopoverContent>
  </Popover>
}

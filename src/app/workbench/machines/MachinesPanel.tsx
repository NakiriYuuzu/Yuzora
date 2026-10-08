import { useEffect, useRef, useState } from "react"
import { useTranslation } from "react-i18next"
import { Plus, RefreshCw } from "lucide-react"
import { Button } from "@/components/ui/button"
import {
  AlertDialog, AlertDialogAction, AlertDialogCancel, AlertDialogContent, AlertDialogDescription,
  AlertDialogFooter, AlertDialogHeader, AlertDialogTitle
} from "@/components/ui/alert-dialog"
import { Empty, EmptyDescription, EmptyHeader, EmptyTitle } from "@/components/ui/empty"
import { describeMachineError, parseMachineError } from "@/lib/machinesErrors"
import type { HerdrMachine } from "@/lib/machinesTypes"
import { isWindowsPlatform } from "@/lib/platform"
import { useMachinesInteractiveStore, type MachineInteractiveSelection } from "@/state/machinesInteractiveStore"
import { useMachinesStore } from "@/state/machinesStore"
import { useUiStore } from "@/state/uiStore"
import { MachineAddDialog, type MachineAddValues } from "./MachineAddDialog"
import { MachineRow, type MachineRowStatus } from "./MachineRow"

/** Machines tab of the Session picker. `onClose` closes the picker so only one dialog owns focus. */
export function MachinesPanel({ onClose }: { onClose: () => void }) {
  const { t } = useTranslation("machines")
  const capabilities = useMachinesStore((state) => state.capabilities)
  const capabilitiesError = useMachinesStore((state) => state.capabilitiesError)
  const machines = useMachinesStore((state) => state.machines)
  const listError = useMachinesStore((state) => state.listError)
  const loading = useMachinesStore((state) => state.loading)
  const statusById = useMachinesStore((state) => state.statusById)
  const snapshotById = useMachinesStore((state) => state.snapshotById)
  const staleById = useMachinesStore((state) => state.staleById)
  const errorById = useMachinesStore((state) => state.errorById)
  const [adding, setAdding] = useState(false)
  const [removing, setRemoving] = useState<HerdrMachine | null>(null)
  const [busyIds, setBusyIds] = useState<ReadonlySet<string>>(new Set())
  const [actionError, setActionError] = useState<string | null>(null)
  const mounted = useRef(true)
  const windows = isWindowsPlatform()
  const t2 = (key: string, options?: Record<string, unknown>) => t(key, options) as string

  useEffect(() => {
    mounted.current = true
    void (async () => {
      const store = useMachinesStore.getState()
      const loaded = await store.loadCapabilities()
      if (loaded?.supported && mounted.current) {
        await store.refreshList()
        // Opening the tab must not clear auth-required blocks or backoff: only a soft round.
        store.requestRefresh(false)
      }
    })()
    return () => { mounted.current = false }
  }, [])

  async function guarded(id: string | null, action: () => Promise<unknown>) {
    const setBusy = (on: boolean) => {
      if (!id || !mounted.current) return
      setBusyIds((current) => {
        const next = new Set(current)
        if (on) next.add(id); else next.delete(id)
        return next
      })
    }
    setBusy(true)
    setActionError(null)
    try { await action() }
    catch (cause) { if (mounted.current) setActionError(describeMachineError(cause, t2).message) }
    finally { setBusy(false) }
  }
  function openInteractive(selection: MachineInteractiveSelection) {
    if (!useMachinesInteractiveStore.getState().open(selection)) {
      setActionError(t("interactiveAlreadyOpen"))
      return
    }
    onClose()
  }
  function submitAdd(values: MachineAddValues) {
    setAdding(false)
    openInteractive({
      spec: {
        kind: "add",
        target: values.target,
        ...(values.remoteSession ? { remoteSession: values.remoteSession } : {}),
        ...(values.label ? { label: values.label } : {})
      }
    })
  }
  function rowStatus(machine: HerdrMachine): MachineRowStatus {
    if (!machine.enabled) return "disabled"
    const checked = statusById[machine.id]
    const code = errorById[machine.id] ? parseMachineError(errorById[machine.id]).code : null
    if (code === "machines-auth-required") return "auth-required"
    if (errorById[machine.id]) return "error"
    if (checked) return checked.status
    return snapshotById[machine.id] ? "reachable" : "unknown"
  }
  function rowError(machine: HerdrMachine): string | null {
    const raw = errorById[machine.id] ?? (statusById[machine.id]?.status === "error" ? statusById[machine.id]?.error : null)
    return raw ? describeMachineError(raw, t2).message : null
  }

  if (!capabilities && capabilitiesError) {
    return <Empty>
      <EmptyHeader>
        <EmptyTitle role="alert">{t("unsupported.probeFailed")}</EmptyTitle>
        <EmptyDescription>{describeMachineError(capabilitiesError, t2).message}</EmptyDescription>
      </EmptyHeader>
      <Button variant="outline" onClick={() => void useMachinesStore.getState().loadCapabilities()}>{t("panel.retry")}</Button>
    </Empty>
  }
  if (capabilities && !capabilities.supported) {
    const binaryMissing = capabilities.reason === "machines-binary-unavailable"
    return <Empty>
      <EmptyHeader>
        <EmptyTitle role="status">{binaryMissing ? t("unsupported.binaryUnavailableTitle") : t("unsupported.title")}</EmptyTitle>
        <EmptyDescription>{binaryMissing
          ? describeMachineError(capabilities.reason, t2).message
          : t("unsupported.description", { version: capabilities.version ?? t("unsupported.unknownVersion") })}</EmptyDescription>
      </EmptyHeader>
      <Button variant="outline" onClick={() => { onClose(); useUiStore.getState().openSettings("herdr") }}>{t("unsupported.openSettings")}</Button>
    </Empty>
  }
  const canReconnect = !windows && (capabilities?.hasReconnect ?? false)
  return <div className="flex flex-col gap-3">
    <p className="herdr-session-picker-hint">{t("panel.description")}</p>
    <div className="flex gap-2">
      <Button size="sm" onClick={() => setAdding(true)}><Plus data-icon="inline-start" />{t("panel.add")}</Button>
      <Button size="sm" variant="ghost" disabled={loading} onClick={() => { void useMachinesStore.getState().refreshList(); useMachinesStore.getState().requestRefresh() }}>
        <RefreshCw data-icon="inline-start" />{t("panel.refresh")}
      </Button>
    </div>
    {windows && <p className="text-xs text-muted-foreground">{t("panel.windowsReconnectHint")}</p>}
    {(listError ?? actionError) && <div role="alert" className="text-sm text-destructive [overflow-wrap:anywhere]">
      <p>{listError ? t("panel.loadFailed") : actionError}</p>
      {listError && <>
        <p>{describeMachineError(listError, t2).message}</p>
        <Button size="sm" variant="outline" onClick={() => void useMachinesStore.getState().refreshList()}>{t("panel.retry")}</Button>
      </>}
    </div>}
    {!machines.length && !listError && (loading
      ? <p role="status">{t("panel.loading")}</p>
      : <Empty><EmptyHeader><EmptyTitle role="status">{t("panel.empty")}</EmptyTitle><EmptyDescription>{t("panel.emptyHint")}</EmptyDescription></EmptyHeader></Empty>)}
    <ul className="flex flex-col gap-2" aria-label={t("panel.title")}>
      {machines.map((machine) => <MachineRow
        key={machine.id}
        machine={machine}
        status={rowStatus(machine)}
        stale={Boolean(staleById[machine.id])}
        errorMessage={rowError(machine)}
        canReconnect={canReconnect}
        busy={busyIds.has(machine.id)}
        actions={{
          onRename: (label) => void guarded(machine.id, () => useMachinesStore.getState().rename(machine.id, label)),
          onToggleEnabled: () => void guarded(machine.id, () => useMachinesStore.getState().setEnabled(machine.id, !machine.enabled)),
          onRemove: () => setRemoving(machine),
          onCheckStatus: () => void guarded(machine.id, () => useMachinesStore.getState().refreshStatus(machine.id)),
          onReconnect: () => openInteractive({ spec: { kind: "reconnect", machineId: machine.id }, machineLabel: machine.label }),
          onOpenClient: () => openInteractive({ spec: { kind: "client" }, machineLabel: machine.label })
        }}
      />)}
    </ul>
    {adding && <MachineAddDialog onSubmit={submitAdd} onCancel={() => setAdding(false)} />}
    <AlertDialog open={removing !== null} onOpenChange={(open) => { if (!open) setRemoving(null) }}>
      <AlertDialogContent>
        <AlertDialogHeader>
          <AlertDialogTitle>{t("remove.title", { label: removing?.label ?? "" })}</AlertDialogTitle>
          <AlertDialogDescription>{t("remove.description")}</AlertDialogDescription>
        </AlertDialogHeader>
        <AlertDialogFooter>
          <AlertDialogCancel>{t("remove.cancel")}</AlertDialogCancel>
          <AlertDialogAction onClick={() => {
            const target = removing
            setRemoving(null)
            if (target) void guarded(target.id, () => useMachinesStore.getState().remove(target.id))
          }}>{t("remove.confirm")}</AlertDialogAction>
        </AlertDialogFooter>
      </AlertDialogContent>
    </AlertDialog>
  </div>
}

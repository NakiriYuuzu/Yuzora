import { useState } from "react"
import { useTranslation } from "react-i18next"
import { Badge } from "@/components/ui/badge"
import { Button } from "@/components/ui/button"
import { Input } from "@/components/ui/input"
import type { HerdrMachine, HerdrMachineStatusKind } from "@/lib/machinesTypes"

export type MachineRowStatus = HerdrMachineStatusKind | "unknown"

export interface MachineRowActions {
  onRename: (label: string) => void
  onToggleEnabled: () => void
  onRemove: () => void
  onCheckStatus: () => void
  onReconnect: () => void
  onOpenClient: () => void
}

export function MachineRow({ machine, status, stale, errorMessage, canReconnect, busy, actions }: {
  machine: HerdrMachine
  status: MachineRowStatus
  stale: boolean
  errorMessage: string | null
  /** False hides Reconnect (Windows, or an old binary without the subcommand). */
  canReconnect: boolean
  busy: boolean
  actions: MachineRowActions
}) {
  const { t } = useTranslation("machines")
  const [renaming, setRenaming] = useState(false)
  const [draft, setDraft] = useState(machine.label)
  function commitRename() {
    const label = draft.trim()
    setRenaming(false)
    if (label && label !== machine.label) actions.onRename(label)
  }
  return <li className="flex flex-col gap-2 rounded-md border p-3" data-machine-id={machine.id} data-enabled={machine.enabled}>
    <div className="flex min-w-0 items-center gap-2">
      {renaming ? <form className="flex min-w-0 flex-1 items-center gap-2" onSubmit={(event) => { event.preventDefault(); commitRename() }}>
        <Input aria-label={t("actions.renameLabel")} value={draft} autoFocus onChange={(event) => setDraft(event.target.value)}
          onKeyDown={(event) => { if (event.key === "Escape") { event.stopPropagation(); setRenaming(false) } }} />
        <Button type="submit" size="sm" disabled={!draft.trim()}>{t("actions.renameSave")}</Button>
        <Button type="button" size="sm" variant="ghost" onClick={() => setRenaming(false)}>{t("actions.renameCancel")}</Button>
      </form> : <>
        <strong className="min-w-0 truncate" title={machine.label}>{machine.label}</strong>
        <Badge variant="outline" data-status={status}>{t(`status.${status}`)}</Badge>
        {stale && <Badge variant="outline" data-stale="true">{t("status.stale")}</Badge>}
      </>}
    </div>
    <p className="min-w-0 truncate font-mono text-xs text-muted-foreground" title={`${machine.target} · ${machine.session}`}>
      {machine.target} · {machine.session}
    </p>
    {errorMessage && <p role="status" className="text-xs text-destructive [overflow-wrap:anywhere]">{errorMessage}</p>}
    <div className="flex flex-wrap gap-1" role="group" aria-label={t("row.actions", { label: machine.label })}>
      <Button size="sm" variant="ghost" disabled={busy} onClick={() => { setDraft(machine.label); setRenaming(true) }}>{t("actions.rename")}</Button>
      <Button size="sm" variant="ghost" disabled={busy} onClick={actions.onToggleEnabled}>{t(machine.enabled ? "actions.disable" : "actions.enable")}</Button>
      <Button size="sm" variant="ghost" disabled={busy || !machine.enabled} onClick={actions.onCheckStatus}>{t("actions.checkStatus")}</Button>
      {canReconnect && <Button size="sm" variant="ghost" disabled={busy || !machine.enabled} onClick={actions.onReconnect}>{t("actions.reconnect")}</Button>}
      <Button size="sm" variant="ghost" disabled={busy || !machine.enabled} onClick={actions.onOpenClient}>{t("actions.openClient")}</Button>
      <Button size="sm" variant="ghost" disabled={busy} onClick={actions.onRemove}>{t("actions.remove")}</Button>
    </div>
  </li>
}

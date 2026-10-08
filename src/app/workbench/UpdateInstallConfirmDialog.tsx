import { useEffect, useState } from "react"
import { useTranslation } from "react-i18next"
import {
  AlertDialog,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
} from "@/components/ui/alert-dialog"
import { Button } from "@/components/ui/button"
import {
  updateHerdrProcesses,
  updateStopHerdr,
  type UpdateHerdrProcesses,
} from "@/lib/updateChannel"

interface UpdateInstallConfirmDialogProps {
  open: boolean
  onOpenChange: (open: boolean) => void
  /** Windows installers cannot replace a running HERDR, so it is stopped first. */
  stopHerdr: boolean
  onConfirm: () => void
}

/** Mount with a fresh `key` per request so each confirmation re-reads HERDR. */
export function UpdateInstallConfirmDialog({
  open,
  onOpenChange,
  stopHerdr,
  onConfirm,
}: UpdateInstallConfirmDialogProps) {
  const { t: tw } = useTranslation("workbench")
  const [herdr, setHerdr] = useState<UpdateHerdrProcesses | null>(null)
  const [herdrCheckFailed, setHerdrCheckFailed] = useState(false)
  const [stopping, setStopping] = useState(false)
  const [stopError, setStopError] = useState<string | null>(null)
  const herdrChecking = stopHerdr && herdr === null && !herdrCheckFailed
  // Only a user-installed HERDR is running: nothing bundled to stop, so no kill warning or stop call.
  const externalOnly = stopHerdr && herdr !== null && herdr.pids.length === 0 && !!herdr.external
  const willStop = stopHerdr && !externalOnly

  useEffect(() => {
    if (!open || !stopHerdr) return
    let active = true
    void updateHerdrProcesses()
      .then((processes) => {
        if (active) setHerdr(processes)
      })
      .catch(() => {
        if (active) setHerdrCheckFailed(true)
      })
    return () => {
      active = false
    }
  }, [open, stopHerdr])

  const confirm = async () => {
    if (willStop) {
      setStopping(true)
      setStopError(null)
      try {
        await updateStopHerdr()
      } catch (error) {
        setStopError(String(error))
        return
      } finally {
        setStopping(false)
      }
    }
    onConfirm()
  }

  return (
    <AlertDialog
      open={open}
      onOpenChange={(next) => {
        if (!stopping) onOpenChange(next)
      }}
    >
      <AlertDialogContent className="sm:max-w-[460px]">
        <AlertDialogHeader>
          <AlertDialogTitle>{tw("settings.installConfirmTitle")}</AlertDialogTitle>
          <AlertDialogDescription>{tw("settings.installConfirmDescription")}</AlertDialogDescription>
        </AlertDialogHeader>
        {willStop && (
          <div className="grid gap-2 rounded-lg border border-destructive/30 bg-destructive/5 p-3 text-[12px]">
            <p className="font-medium text-destructive">{tw("settings.installConfirmHerdrWarning")}</p>
            {herdrChecking ? (
              <p className="text-muted-foreground">{tw("settings.installConfirmHerdrChecking")}</p>
            ) : herdrCheckFailed ? (
              <p className="text-muted-foreground">{tw("settings.installConfirmHerdrCheckFailed")}</p>
            ) : (
              herdr && (
                <dl className="grid grid-cols-[auto_minmax(0,1fr)] gap-x-3 gap-y-1">
                  <dt className="text-muted-foreground">{tw("settings.installConfirmHerdrVersion")}</dt>
                  <dd>{herdr.version ?? tw("settings.installConfirmHerdrUnknown")}</dd>
                  <dt className="text-muted-foreground">{tw("settings.installConfirmHerdrPath")}</dt>
                  <dd className="font-mono break-all">
                    {herdr.path ?? tw("settings.installConfirmHerdrUnknown")}
                  </dd>
                  <dt className="text-muted-foreground">{tw("settings.installConfirmHerdrProcesses")}</dt>
                  <dd>{herdr.pids.length}</dd>
                </dl>
              )
            )}
          </div>
        )}
        {stopHerdr && herdr?.external && (
          <div data-testid="herdr-external-note" className="grid gap-2 rounded-lg border bg-muted/40 p-3 text-[12px]">
            <p className="font-medium">{tw("settings.installConfirmHerdrExternalTitle")}</p>
            <p className="text-muted-foreground">{tw("settings.installConfirmHerdrExternalNote")}</p>
            <dl className="grid grid-cols-[auto_minmax(0,1fr)] gap-x-3 gap-y-1">
              <dt className="text-muted-foreground">{tw("settings.installConfirmHerdrPath")}</dt>
              <dd className="font-mono break-all">{herdr.external.path}</dd>
              <dt className="text-muted-foreground">{tw("settings.installConfirmHerdrExternalPids")}</dt>
              <dd>{herdr.external.pids.length ? herdr.external.pids.join(", ") : tw("settings.installConfirmHerdrUnknown")}</dd>
            </dl>
          </div>
        )}
        {stopError && (
          <p role="alert" className="text-[12px] text-destructive">
            {tw("settings.stopHerdrFailed", { error: stopError })}
          </p>
        )}
        <AlertDialogFooter>
          <AlertDialogCancel disabled={stopping}>{tw("settings.cancelInstall")}</AlertDialogCancel>
          <Button
            variant={willStop ? "destructive" : "default"}
            disabled={stopping || herdrChecking}
            onClick={() => void confirm()}
          >
            {stopping
              ? tw("settings.stoppingHerdr")
              : willStop
                ? tw("settings.stopHerdrAndInstall")
                : tw("settings.installAndRestart")}
          </Button>
        </AlertDialogFooter>
      </AlertDialogContent>
    </AlertDialog>
  )
}

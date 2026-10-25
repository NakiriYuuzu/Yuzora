import { useEffect, useState } from "react"
import { useTranslation } from "react-i18next"
import { Bell } from "lucide-react"
import { AlertDialog, AlertDialogContent, AlertDialogDescription, AlertDialogFooter, AlertDialogHeader, AlertDialogTitle } from "@/components/ui/alert-dialog"
import { Button } from "@/components/ui/button"
import { rpcErrorKey } from "../lib/errors"
import { RpcError } from "../lib/rpc"
import { disablePush, enablePush, pushState, watchInstalling, type PushState } from "../push"
import { applyUpdate } from "../serviceWorker"
import { Message } from "./Page"

/** The bell that opens the notifications dialog. */
export function PushControl({ vapidPublicKey }: { vapidPublicKey: string }) {
  const { t } = useTranslation()
  const [shown, setShown] = useState(false)
  return (
    <>
      <Button variant="ghost" size="icon" className="size-11" aria-label={t("push.title")} onClick={() => setShown(true)}>
        <Bell aria-hidden />
      </Button>
      {shown && <PushDialog vapidPublicKey={vapidPublicKey} onClose={() => setShown(false)} />}
    </>
  )
}

const pushFailure = (error: unknown) => (error instanceof RpcError ? rpcErrorKey(error.code) : "push.failed")

/** Mount it only while shown: it reads the state on mount. Turning on happens in the tap on its button. */
function PushDialog({ vapidPublicKey, onClose }: { vapidPublicKey: string; onClose: () => void }) {
  const { t } = useTranslation()
  const [state, setState] = useState<PushState | null>(null)
  const [failure, setFailure] = useState<string | null>(null)
  const [busy, setBusy] = useState(false)
  // Bumped to read the state again from the start.
  const [reads, setReads] = useState(0)

  useEffect(() => {
    let live = true
    let latest = 0
    let stop = () => {}
    const read = () => {
      const mine = ++latest
      pushState(vapidPublicKey).then(
        (next) => {
          if (!live || mine !== latest) return
          setState(next)
          // A worker still installing settles into another state: read again then.
          stop()
          if (next === "updating") stop = watchInstalling(read)
        },
        (error: unknown) => live && mine === latest && setFailure(pushFailure(error)),
      )
    }
    read()
    return () => {
      live = false
      stop()
    }
  }, [vapidPublicKey, reads])

  // Called from a tap: the action starts before anything is awaited.
  const run = (action: () => Promise<PushState>) => {
    setBusy(true)
    setFailure(null)
    action()
      .then(
        (next) => {
          setState(next)
          // An update began installing after the dialog read the state: follow it from here.
          if (next === "updating") setReads((count) => count + 1)
        },
        (error: unknown) => setFailure(pushFailure(error)),
      )
      .finally(() => setBusy(false))
  }

  return (
    <AlertDialog open onOpenChange={(open) => !open && !busy && onClose()}>
      <AlertDialogContent>
        <AlertDialogHeader>
          <AlertDialogTitle className="text-lg">{t("push.title")}</AlertDialogTitle>
          <AlertDialogDescription className="text-base">{state ? t(`push.state.${state}`) : !failure && t("push.checking")}</AlertDialogDescription>
        </AlertDialogHeader>
        {failure && <Message tone="error">{t(failure)}</Message>}
        <AlertDialogFooter className="flex-row justify-end gap-2">
          <Button variant="outline" className="h-11" disabled={busy} onClick={onClose}>{t("common.dismiss")}</Button>
          {state === "off" && <Button className="h-11" disabled={busy} onClick={() => run(() => enablePush(vapidPublicKey))}>{t("push.enable")}</Button>}
          {state === "update-first" && <Button className="h-11" disabled={busy} onClick={applyUpdate}>{t("update.reload")}</Button>}
          {state === "on" && <Button className="h-11" disabled={busy} onClick={() => run(disablePush)}>{t("push.disable")}</Button>}
        </AlertDialogFooter>
      </AlertDialogContent>
    </AlertDialog>
  )
}

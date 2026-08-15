import { useTranslation } from "react-i18next"

import {
  AlertDialog,
  AlertDialogAction,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
} from "@/components/ui/alert-dialog"
import { ScrollArea } from "@/components/ui/scroll-area"
import { useAppDialogStore } from "@/state/appDialogStore"

/**
 * Compact app-owned AlertDialog host. Explicitly excluded from the 80%-viewport
 * resizable Dialog contract — alerts remain content-sized and non-resizable.
 */
export function AppDialogHost() {
  const { t } = useTranslation("menus")
  const pending = useAppDialogStore((state) => state.pending)
  const respond = useAppDialogStore((state) => state.respond)

  return (
    <AlertDialog
      open={pending !== null}
      onOpenChange={(open) => {
        if (!open) respond(false)
      }}
    >
      {pending && (
        <AlertDialogContent data-testid="app-dialog-content">
          <AlertDialogHeader>
            <AlertDialogTitle>{pending.title}</AlertDialogTitle>
          </AlertDialogHeader>
          {/* Bounded scroll for long runtime/OS errors without forcing 80% sizing. */}
          <ScrollArea
            data-testid="app-dialog-body"
            className="max-h-[40vh]"
            viewportClassName="pr-1"
          >
            <AlertDialogDescription className="break-words whitespace-pre-wrap [overflow-wrap:anywhere]">
              {pending.description}
            </AlertDialogDescription>
          </ScrollArea>
          <AlertDialogFooter>
            {pending.type === "confirm" && (
              <AlertDialogCancel onClick={() => respond(false)}>
                {pending.cancelLabel ?? t("appDialog.cancel")}
              </AlertDialogCancel>
            )}
            <AlertDialogAction
              variant={
                pending.type === "confirm" && pending.destructive
                  ? "destructive"
                  : "default"
              }
              onClick={() => respond(true)}
            >
              {pending.confirmLabel ??
                (pending.type === "message"
                  ? t("appDialog.ok")
                  : t("appDialog.confirm"))}
            </AlertDialogAction>
          </AlertDialogFooter>
        </AlertDialogContent>
      )}
    </AlertDialog>
  )
}

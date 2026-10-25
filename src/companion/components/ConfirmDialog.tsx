import type { ReactNode } from "react"
import { useTranslation } from "react-i18next"
import { AlertDialog, AlertDialogContent, AlertDialogDescription, AlertDialogFooter, AlertDialogHeader, AlertDialogTitle } from "@/components/ui/alert-dialog"
import { Button } from "@/components/ui/button"

/** A controlled confirmation that stays open while its async action runs. */
export function ConfirmDialog({ title, description, children, confirmLabel, busy = false, disabled = false, onConfirm, onCancel }: {
  title: string
  description?: ReactNode
  children?: ReactNode
  confirmLabel: string
  busy?: boolean
  disabled?: boolean
  onConfirm: () => void
  onCancel: () => void
}) {
  const { t } = useTranslation()
  return (
    <AlertDialog open onOpenChange={(open) => !open && !busy && onCancel()}>
      <AlertDialogContent>
        <AlertDialogHeader>
          <AlertDialogTitle className="text-lg">{title}</AlertDialogTitle>
          {description && <AlertDialogDescription className="text-base">{description}</AlertDialogDescription>}
        </AlertDialogHeader>
        {children}
        <AlertDialogFooter className="flex-row justify-end gap-2">
          <Button variant="outline" className="h-11" disabled={busy} onClick={onCancel}>{t("common.cancel")}</Button>
          <Button className="h-11" disabled={busy || disabled} onClick={onConfirm}>{confirmLabel}</Button>
        </AlertDialogFooter>
      </AlertDialogContent>
    </AlertDialog>
  )
}

import { useState, type FormEvent } from "react"
import { useTranslation } from "react-i18next"
import { Button } from "@/components/ui/button"
import { Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle } from "@/components/ui/dialog"
import { Field, FieldGroup, FieldLabel } from "@/components/ui/field"
import { Input } from "@/components/ui/input"

export interface MachineAddValues { target: string; label: string; remoteSession: string }

/** Collects the SSH target; session choice and install prompts happen in the interactive terminal. */
export function MachineAddDialog({ title, description, initial, notice, onSubmit, onCancel }: {
  title?: string
  description?: string
  initial?: Partial<MachineAddValues>
  /** Extra warning shown above the form (e.g. key-file note when migrating an SSH host). */
  notice?: string
  onSubmit: (values: MachineAddValues) => void
  onCancel: () => void
}) {
  const { t } = useTranslation("machines")
  const [target, setTarget] = useState(initial?.target ?? "")
  const [label, setLabel] = useState(initial?.label ?? "")
  const [remoteSession, setRemoteSession] = useState(initial?.remoteSession ?? "")
  const valid = target.trim().length > 0
  function submit(event: FormEvent) {
    event.preventDefault()
    if (valid) onSubmit({ target: target.trim(), label: label.trim(), remoteSession: remoteSession.trim() })
  }
  return <Dialog open onOpenChange={(open) => { if (!open) onCancel() }}>
    <DialogContent className="sm:max-w-[480px]">
      <form onSubmit={submit} className="flex flex-col gap-4">
        <DialogHeader>
          <DialogTitle>{title ?? t("add.title")}</DialogTitle>
          <DialogDescription>{description ?? t("add.description")}</DialogDescription>
        </DialogHeader>
        {notice && <p role="note" className="text-sm text-amber-600 dark:text-amber-400">{notice}</p>}
        <FieldGroup>
          <Field>
            <FieldLabel htmlFor="machine-add-target">{t("add.target")}</FieldLabel>
            <Input id="machine-add-target" value={target} autoFocus spellCheck={false} placeholder={t("add.targetPlaceholder")} onChange={(event) => setTarget(event.target.value)} />
          </Field>
          <Field>
            <FieldLabel htmlFor="machine-add-label">{t("add.label")}</FieldLabel>
            <Input id="machine-add-label" value={label} onChange={(event) => setLabel(event.target.value)} />
          </Field>
          <Field>
            <FieldLabel htmlFor="machine-add-session">{t("add.session")}</FieldLabel>
            <Input id="machine-add-session" value={remoteSession} spellCheck={false} placeholder={t("add.sessionPlaceholder")} onChange={(event) => setRemoteSession(event.target.value)} />
          </Field>
        </FieldGroup>
        <p className="text-xs text-muted-foreground">{t("panel.sshAgentHint")}</p>
        <DialogFooter>
          <Button type="button" variant="ghost" onClick={onCancel}>{t("add.cancel")}</Button>
          <Button type="submit" disabled={!valid}>{t("add.submit")}</Button>
        </DialogFooter>
      </form>
    </DialogContent>
  </Dialog>
}

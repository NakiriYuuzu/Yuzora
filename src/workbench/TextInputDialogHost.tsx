import { useState } from "react"
import { useTranslation } from "react-i18next"

import { Button } from "@/components/ui/button"
import {
    Dialog,
    DialogContent,
    DialogDescription,
    DialogFooter,
    DialogHeader,
    DialogTitle
} from "@/components/ui/dialog"
import { Field, FieldLabel } from "@/components/ui/field"
import { Input } from "@/components/ui/input"
import { useTextInputDialogStore } from "@/state/textInputDialogStore"

export function TextInputDialogHost() {
    const pending = useTextInputDialogStore((state) => state.pending)
    const respond = useTextInputDialogStore((state) => state.respond)

    return (
        <Dialog
            open={pending !== null}
            onOpenChange={(open) => {
                if (!open) respond(null)
            }}
        >
            {pending && (
                <TextInputDialogForm
                    key={pending.requestId}
                    pending={pending}
                    onRespond={respond}
                />
            )}
        </Dialog>
    )
}

function TextInputDialogForm({
    pending,
    onRespond
}: {
    pending: NonNullable<ReturnType<typeof useTextInputDialogStore.getState>["pending"]>
    onRespond: (value: string | null) => void
}) {
    const { t } = useTranslation("menus")
    const [value, setValue] = useState(pending.initialValue ?? "")
    const trimmed = value.trim()

    return (
        <DialogContent
            showCloseButton={false}
            className="flex min-h-0 flex-col"
        >
            <form
                className="flex min-h-0 flex-1 flex-col"
                onSubmit={(event) => {
                    event.preventDefault()
                    if (trimmed) onRespond(trimmed)
                }}
            >
                <DialogHeader>
                    <DialogTitle>{pending.title}</DialogTitle>
                    {pending.description && (
                        <DialogDescription>{pending.description}</DialogDescription>
                    )}
                </DialogHeader>
                <Field className="mt-4 min-h-0 flex-1">
                    <FieldLabel htmlFor="text-input-dialog-value">{pending.label}</FieldLabel>
                    <Input
                        id="text-input-dialog-value"
                        autoFocus
                        value={value}
                        placeholder={pending.placeholder}
                        onChange={(event) => setValue(event.target.value)}
                        onFocus={(event) => event.currentTarget.select()}
                    />
                </Field>
                <DialogFooter className="mt-4">
                    <Button type="button" variant="ghost" onClick={() => onRespond(null)}>
                        {t("textInputDialog.cancel")}
                    </Button>
                    <Button type="submit" disabled={!trimmed}>
                        {pending.confirmLabel}
                    </Button>
                </DialogFooter>
            </form>
        </DialogContent>
    )
}

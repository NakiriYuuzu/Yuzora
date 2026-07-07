import { useTranslation } from "react-i18next"

import {
    Dialog,
    DialogContent,
    DialogDescription,
    DialogFooter,
    DialogHeader,
    DialogTitle
} from "@/components/ui/dialog"
import { Button } from "@/components/ui/button"
import { useConfirmDialogStore } from "@/state/confirmDialogStore"

/**
 * Shared "unsaved changes" confirmation modal driven by confirmDialogStore. Both
 * the TabBar close flow and the workspace-switch flow await
 * requestUnsavedDecision(); this host resolves that promise from the button the
 * user presses. Escape / overlay dismiss (onOpenChange → false) counts as
 * "cancel".
 */
export function ConfirmDialogHost() {
    const { t } = useTranslation("menus")
    const pending = useConfirmDialogStore((s) => s.pending)
    const respond = useConfirmDialogStore((s) => s.respond)

    return (
        <Dialog
            open={pending !== null}
            onOpenChange={(open) => {
                if (!open) respond("cancel")
            }}
        >
            {pending && (
                <DialogContent showCloseButton={false} className="sm:max-w-[420px]">
                    <DialogHeader>
                        <DialogTitle>{pending.title}</DialogTitle>
                        <DialogDescription>{pending.description}</DialogDescription>
                    </DialogHeader>
                    <DialogFooter>
                        <Button variant="ghost" onClick={() => respond("cancel")}>
                            {t("unsavedDialog.cancel")}
                        </Button>
                        <Button variant="destructive" onClick={() => respond("discard")}>
                            {t("unsavedDialog.discard")}
                        </Button>
                        <Button variant="default" onClick={() => respond("save")}>
                            {pending.saveLabel}
                        </Button>
                    </DialogFooter>
                </DialogContent>
            )}
        </Dialog>
    )
}

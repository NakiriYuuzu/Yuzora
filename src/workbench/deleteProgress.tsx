import { toast } from "sonner"

import type { DeleteProgress } from "@/lib/types"
import { patchDeleteTask, removeDeleteTask, useDeleteTasks } from "@/state/deleteProgressStore"
import { DeleteProgressToast } from "./DeleteProgressToast"

/** Quick deletes finish before this and never flash a toast. */
const TOAST_DELAY_MS = 300

/**
 * Runs a delete with a progress toast in the notification area, shown once the
 * delete outlasts TOAST_DELAY_MS. Local deletes get a determinate bar and a
 * Cancel button; remote ones only an indeterminate bar.
 */
export async function withDeleteProgress(
    name: string,
    local: boolean,
    run: (operation: { id: string; onProgress: (progress: DeleteProgress) => void }) => Promise<void>
): Promise<void> {
    const id = `delete:${crypto.randomUUID()}`
    useDeleteTasks.setState({ [id]: { name, progress: null, local, cancelling: false } })
    const timer = setTimeout(() => {
        toast.custom(() => <DeleteProgressToast id={id} />, { id, duration: Infinity, dismissible: false })
    }, TOAST_DELAY_MS)
    try {
        await run({ id, onProgress: (progress) => patchDeleteTask(id, { progress }) })
    } finally {
        clearTimeout(timer)
        toast.dismiss(id)
        removeDeleteTask(id)
    }
}

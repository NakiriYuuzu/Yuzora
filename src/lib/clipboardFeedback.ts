import { writeText } from "@tauri-apps/plugin-clipboard-manager"
import { toast } from "sonner"
import i18n from "@/lib/i18n"

const COPIED_TOAST_ID = "yuzora-clipboard-copied"

/**
 * Copy a value the user explicitly asked for (path, hash, branch, address…)
 * and confirm it with a short, non-blocking toast. Failures still reject so
 * callers keep their existing error reporting. Editor cut/copy keep using the
 * raw clipboard API: those are editing gestures, not discrete actions.
 */
export async function copyTextWithFeedback(text: string): Promise<void> {
    await writeText(text)
    toast.success(i18n.t("clipboard.copied", { ns: "common" }), { id: COPIED_TOAST_ID, duration: 1800 })
}

/** Fire-and-forget variant for buttons that have no surrounding error path. */
export function copyTextInBackground(text: string): void {
    void copyTextWithFeedback(text).catch(() => {
        toast.error(i18n.t("clipboard.copyFailed", { ns: "common" }), { id: COPIED_TOAST_ID })
    })
}

import { message } from "@tauri-apps/plugin-dialog"

import i18n from "@/lib/i18n"

function safeErrorMessage(error: unknown): string {
  if (error instanceof Error && error.message.trim()) return error.message.trim()
  if (typeof error === "string" && error.trim()) return error.trim()
  return i18n.t("contextMenu.error.unknown", { ns: "menus" })
}

export async function showActionError(actionLabel: string, error: unknown): Promise<void> {
  await message(
    i18n.t("contextMenu.error.description", {
      ns: "menus",
      action: actionLabel,
      error: safeErrorMessage(error),
    }),
    {
      title: i18n.t("contextMenu.error.title", { ns: "menus" }),
      kind: "error",
    }
  )
}

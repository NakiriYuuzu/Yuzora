import { useEffect } from "react"
import { listen } from "@tauri-apps/api/event"
import { getCurrentWebview, type DragDropEvent } from "@tauri-apps/api/webview"

import { getDocument } from "@/editor/documentRegistry"
import { logUserAction } from "@/features/logs/userAction"
import { showActionError } from "@/lib/actionFeedback"
import i18n from "@/lib/i18n"
import { isTauri } from "@/lib/platform"
import { useUiStore } from "@/state/uiStore"
import { useWorkspaceStore } from "@/state/workspaceStore"

const OWNED_DROP_TARGET_SELECTOR = "[data-yuzora-os-file-drop-target]"
const PREVIEW_FILE_DROP_EVENT = "preview:file-drop"

interface ForwardedFileDropPayload {
  paths: string[]
}

function dropIsOwnedByAnotherSurface(event: Extract<DragDropEvent, { type: "drop" }>): boolean {
  const dpr = window.devicePixelRatio || 1
  const x = event.position.x / dpr
  const y = event.position.y / dpr
  return Array.from(document.querySelectorAll<HTMLElement>(OWNED_DROP_TARGET_SELECTOR)).some(
    (target) => {
      const rect = target.getBoundingClientRect()
      return x >= rect.left && x <= rect.right && y >= rect.top && y <= rect.bottom
    }
  )
}

async function openDroppedFiles(paths: string[]): Promise<void> {
  if (paths.length === 0) return

  const groupIndex = useWorkspaceStore.getState().activeGroupIndex
  const results = await Promise.allSettled(paths.map((path) => getDocument(path)))
  const acceptedPaths = paths.filter((_, index) => results[index]?.status === "fulfilled")
  const firstFailure = results.find((result) => result.status === "rejected")

  if (acceptedPaths.length > 0) {
    const ui = useUiStore.getState()
    if (ui.mode !== "ade" && ui.mode !== "files") ui.setMode("files")
    const workspace = useWorkspaceStore.getState()
    for (const path of acceptedPaths) workspace.openTab(path, groupIndex)
    void logUserAction("open_dropped_files", `Opened ${acceptedPaths.length} dropped file(s)`, {
      count: acceptedPaths.length,
    })
  }

  if (firstFailure?.status === "rejected") {
    void showActionError(
      i18n.t("fileDrop.openAction", { ns: "workbench" }),
      firstFailure.reason
    )
  }
}

/** Opens Finder/Explorer file drops in Yuzora's existing editable file tabs. */
export function FileDropBridge() {
  useEffect(() => {
    if (!isTauri()) return

    let disposed = false
    let unlistenWebview: (() => void) | undefined
    let unlistenPreview: (() => void) | undefined
    void getCurrentWebview()
      .onDragDropEvent((event) => {
        const payload = event.payload
        if (payload.type !== "drop" || dropIsOwnedByAnotherSurface(payload)) return
        void openDroppedFiles(payload.paths)
      })
      .then((nextUnlisten) => {
        if (disposed) nextUnlisten()
        else unlistenWebview = nextUnlisten
      })
      .catch(() => {})
    void listen<ForwardedFileDropPayload>(PREVIEW_FILE_DROP_EVENT, (event) => {
      void openDroppedFiles(event.payload.paths)
    })
      .then((nextUnlisten) => {
        if (disposed) nextUnlisten()
        else unlistenPreview = nextUnlisten
      })
      .catch(() => {})

    return () => {
      disposed = true
      unlistenWebview?.()
      unlistenPreview?.()
    }
  }, [])

  return null
}

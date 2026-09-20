import { useEffect } from "react"
import { FILE_SAVED_EVENT } from "@/lib/fileSaveEvents"
import { isSameOrDescendantPath } from "@/lib/paths"
import { browserTarget } from "./filePreview"
import { reloadPreview } from "./previewCommands"

/** Coalesce successful saves; never save dirty buffers as a side effect of previewing. */
export function useFilePreviewReload(workspace: string | null, url: string | null, onError: (error: unknown) => Promise<void>) {
  useEffect(() => {
    if (!workspace || !url || browserTarget(url).kind !== "file") return
    let timer: ReturnType<typeof setTimeout> | undefined
    const onSaved = (event: Event) => {
      const path = (event as CustomEvent<string>).detail
      if (!isSameOrDescendantPath(workspace, path)) return
      clearTimeout(timer)
      timer = setTimeout(() => {
        void reloadPreview({ workspacePath: workspace, url }).catch(onError)
      }, 200)
    }
    window.addEventListener(FILE_SAVED_EVENT, onSaved)
    return () => { clearTimeout(timer); window.removeEventListener(FILE_SAVED_EVENT, onSaved) }
  }, [workspace, url, onError])
}

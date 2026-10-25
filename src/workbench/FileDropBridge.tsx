import { useEffect } from "react"
import { listen } from "@tauri-apps/api/event"
import { getCurrentWebview, type DragDropEvent } from "@tauri-apps/api/webview"

import { openExternalFiles } from "@/lib/openExternalFiles"
import { DROP_TARGET_ATTRIBUTE, elementAtPoint } from "@/lib/pointerDrag"
import { isSameOrDescendantPath, nativePathParent } from "@/lib/paths"
import { isTauri } from "@/lib/platform"
import { terminalDropTargetAt } from "@/terminal/terminalDropTargets"
import { notifyTerminalPathPasteError, pastePathsIntoTerminal } from "@/terminal/terminalPathPaste"
import { useWorkspaceStore } from "@/state/workspaceStore"
import { importDroppedFiles } from "./fileClipboard"
import { canImportOsFiles } from "./fileImportTarget"
import { containingFolderRow } from "./fileTreeDom"

const OWNED_DROP_TARGET_SELECTOR = "[data-yuzora-os-file-drop-target]"
const PREVIEW_FILE_DROP_EVENT = "preview:file-drop"

interface ForwardedFileDropPayload {
  paths: string[]
}

function logicalPoint(position: { x: number; y: number }): { x: number; y: number } {
  const dpr = window.devicePixelRatio || 1
  return { x: position.x / dpr, y: position.y / dpr }
}

function dropIsOwnedByAnotherSurface(event: Extract<DragDropEvent, { type: "drop" }>): boolean {
  const { x, y } = logicalPoint(event.position)
  return Array.from(document.querySelectorAll<HTMLElement>(OWNED_DROP_TARGET_SELECTOR)).some(
    (target) => {
      const rect = target.getBoundingClientRect()
      return x >= rect.left && x <= rect.right && y >= rect.top && y <= rect.bottom
    }
  )
}

function terminalLeafAt(position: { x: number; y: number }): Element | null {
  const element = elementAtPoint(logicalPoint(position))
  return terminalDropTargetAt(element) ? element?.closest("[data-attachment-key]") ?? null : null
}

interface TreeDrop {
  /** Folder row or tree root to highlight. */
  element: Element
  workspacePath: string
  dir: string
}

/** Where a drop over the sidebar tree would import: the folder row, the folder of a file row, or the root. SSH workspaces cannot import, so their drops fall through to opening tabs. */
function treeDropAt(position: { x: number; y: number }): TreeDrop | null {
  const hit = elementAtPoint(logicalPoint(position))
  const zone = hit?.closest<HTMLElement>("[data-file-tree-root]")
  const workspacePath = useWorkspaceStore.getState().workspacePath
  if (!zone || !workspacePath || !canImportOsFiles(workspacePath)) return null
  // Filename-filter results are flat file rows: the drop lands in the file's folder.
  const result = hit?.closest<HTMLElement>("[data-file-result-path]")
  if (result) {
    const parent = nativePathParent(result.dataset.fileResultPath!)
    const inside = isSameOrDescendantPath(workspacePath, parent)
    return { element: inside ? result : zone, workspacePath, dir: inside ? parent : workspacePath }
  }
  const row = hit?.closest<HTMLElement>("[data-tree-path]")
  const folder = row?.dataset.treeDir === "true" ? row : row ? containingFolderRow(row) : null
  const dir = folder?.dataset.treePath
  const inside = dir !== undefined && isSameOrDescendantPath(workspacePath, dir)
  return {
    element: inside && folder ? folder : zone,
    workspacePath,
    dir: inside ? dir : workspacePath,
  }
}

/** Imports Finder/Explorer drops over the file tree, pastes them into terminals, and otherwise opens them as editable file tabs. */
export function FileDropBridge() {
  useEffect(() => {
    if (!isTauri()) return

    let disposed = false
    let unlistenWebview: (() => void) | undefined
    let unlistenPreview: (() => void) | undefined
    let indicated: Element | null = null
    const indicate = (leaf: Element | null) => {
      if (leaf === indicated) return
      indicated?.removeAttribute(DROP_TARGET_ATTRIBUTE)
      indicated = leaf
      indicated?.setAttribute(DROP_TARGET_ATTRIBUTE, "inside")
    }
    void getCurrentWebview()
      .onDragDropEvent((event) => {
        const payload = event.payload
        if (payload.type === "enter" || payload.type === "over") {
          indicate(terminalLeafAt(payload.position) ?? treeDropAt(payload.position)?.element ?? null)
          return
        }
        indicate(null)
        if (payload.type !== "drop" || dropIsOwnedByAnotherSurface(payload) || disposed) return
        const terminal = terminalDropTargetAt(elementAtPoint(logicalPoint(payload.position)))
        if (terminal) {
          void pastePathsIntoTerminal(terminal, payload.paths).catch(notifyTerminalPathPasteError)
          return
        }
        const tree = treeDropAt(payload.position)
        if (tree) {
          void importDroppedFiles(tree.workspacePath, tree.dir, payload.paths)
          return
        }
        void openExternalFiles(payload.paths, "drop", () => !disposed)
      })
      .then((nextUnlisten) => {
        if (disposed) nextUnlisten()
        else unlistenWebview = nextUnlisten
      })
      .catch(() => {})
    void listen<ForwardedFileDropPayload>(PREVIEW_FILE_DROP_EVENT, (event) => {
      void openExternalFiles(event.payload.paths, "drop", () => !disposed)
    })
      .then((nextUnlisten) => {
        if (disposed) nextUnlisten()
        else unlistenPreview = nextUnlisten
      })
      .catch(() => {})

    return () => {
      disposed = true
      indicate(null)
      unlistenWebview?.()
      unlistenPreview?.()
    }
  }, [])

  return null
}

import { FolderOpen } from "lucide-react"
import { useTranslation } from "react-i18next"

import { DashedActionButton } from "@/app/workbench/DashedActionButton"
import { EmptyState } from "@/app/workbench/EmptyState"
import { pickWorkspace } from "@/lib/workspaceActions"
import { contextMenuHandler } from "@/state/contextMenuStore"
import { useWorkspaceStore } from "@/state/workspaceStore"
import { FileTree } from "@/workbench/FileTree"

/**
 * Files mode nav content — empty state with an "Open workspace" action when
 * no workspace is open (§8 gap 8 baseline, matching the SQL console empty
 * state); once a workspace is set, renders the file tree (workspace full-text
 * search lives in the ⌘K command palette — PROB-2).
 */
export function FilesNavContent() {
  const { t } = useTranslation("workbench")
  const workspacePath = useWorkspaceStore((state) => state.workspacePath)

  if (!workspacePath) {
    return (
      <div
        className="flex h-full flex-col gap-[10px]"
      >
        <div className="flex flex-1 items-center justify-center">
          <EmptyState
            icon={FolderOpen}
            title={t("files.emptyTitle")}
            description={t("files.emptyDescription")}
          />
        </div>
        <DashedActionButton label={t("files.openWorkspace")} onClick={pickWorkspace} />
      </div>
    )
  }

  return (
    <div
      onContextMenu={contextMenuHandler({ kind: "explorer", workspacePath })}
      className="flex h-full flex-col"
    >
      <div className="flex-1 overflow-y-auto py-[4px]">
        <FileTree />
      </div>
    </div>
  )
}

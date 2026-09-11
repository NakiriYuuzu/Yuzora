import { FolderOpen } from "lucide-react"
import { useTranslation } from "react-i18next"

import { EmptyState } from "@/app/workbench/EmptyState"
import { Alert, AlertDescription } from "@/components/ui/alert"
import { Button } from "@/components/ui/button"
import { ScrollArea } from "@/components/ui/scroll-area"
import { FileIcon } from "@/lib/fileIcons"
import { relativePathWithin } from "@/lib/paths"
import { cn } from "@/lib/utils"
import { contextMenuHandler } from "@/state/contextMenuStore"
import { useWorkspaceStore } from "@/state/workspaceStore"
import { FileTree } from "@/workbench/FileTree"
import { useFileNameSearch } from "@/workbench/search/useFileNameSearch"

/** File browsing stays mounted while the card switches tools or filters names. */
export function FilesNavContent({ filterQuery = "", active = true }: { filterQuery?: string; active?: boolean }) {
  const { t } = useTranslation("workbench")
  const workspacePath = useWorkspaceStore((state) => state.workspacePath)
  const treeRevision = useWorkspaceStore((state) => state.treeRevision)
  const sourceGroupIndex = useWorkspaceStore((state) => state.activeGroupIndex)
  const openTab = useWorkspaceStore((state) => state.openTab)
  const filtering = filterQuery.trim().length > 0
  const { files, loading, incomplete, error } = useFileNameSearch(workspacePath, filterQuery, treeRevision, active)

  return (
    <div
      onContextMenu={workspacePath ? contextMenuHandler({ kind: "explorer", workspacePath }) : undefined}
      className="flex h-full min-w-0 flex-col"
    >
      {workspacePath ? (
        <>
          <ScrollArea className={cn("min-h-0 flex-1", filtering && "hidden")} aria-hidden={filtering} inert={filtering} viewportClassName="py-1">
            <FileTree />
          </ScrollArea>
          {filtering && (
            <ScrollArea key={`${workspacePath}:${filterQuery}`} className="min-h-0 flex-1" viewportClassName="py-1" viewportProps={{ "aria-busy": loading }}>
              <div className="flex flex-col gap-2">
                {error && <Alert variant="destructive"><AlertDescription>{t("files.searchFailed")}</AlertDescription></Alert>}
                {incomplete && <Alert><AlertDescription>{t("files.searchIncomplete")}</AlertDescription></Alert>}
                <p role="status" className="px-2 text-xs text-muted-foreground">
                  {loading ? t("files.searching") : files.length ? t("files.searchCount", { count: files.length }) : !error ? t("files.searchEmpty") : null}
                </p>
                <ul aria-label={t("files.searchResults")} className="flex min-w-0 flex-col gap-1">
                  {files.map((file) => {
                    const relativePath = relativePathWithin(workspacePath, file.path) ?? file.path
                    return (
                      <li key={file.path} className="min-w-0">
                        <Button variant="ghost" className="h-auto w-full min-w-0 justify-start py-2" title={relativePath}
                          aria-label={relativePath} onClick={() => openTab(file.path)}
                          onContextMenu={contextMenuHandler({ kind: "file", workspacePath, path: file.path, isDirectory: false, sourceGroupIndex })}>
                          <FileIcon fileName={file.name} className="size-4 shrink-0" />
                          <span className="flex min-w-0 flex-col items-start gap-0.5">
                            <span className="max-w-full truncate">{file.name}</span>
                            {relativePath !== file.name && <span className="max-w-full truncate text-xs text-muted-foreground">{relativePath}</span>}
                          </span>
                        </Button>
                      </li>
                    )
                  })}
                </ul>
              </div>
            </ScrollArea>
          )}
        </>
      ) : (
        <div className="flex flex-1 items-center justify-center">
          <EmptyState icon={FolderOpen} title={t("files.emptyTitle")} description={t("files.selectWorkspaceHint")} />
        </div>
      )}
    </div>
  )
}

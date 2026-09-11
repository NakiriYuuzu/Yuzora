import { useState } from "react"
import { Copy, FilePlus2, FolderOpen, FolderPlus, ListFilter, MoreHorizontal, RefreshCw } from "lucide-react"
import { useTranslation } from "react-i18next"
import { Button } from "@/components/ui/button"
import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card"
import { InputGroup, InputGroupAddon, InputGroupInput } from "@/components/ui/input-group"
import { DropdownMenu, DropdownMenuContent, DropdownMenuGroup, DropdownMenuItem, DropdownMenuTrigger } from "@/components/ui/dropdown-menu"
import { Tabs, TabsContent, TabsList, TabsTrigger } from "@/components/ui/tabs"
import { executeLegacyContextMenuAction, runContextMenuAction } from "@/state/contextMenuStore"
import { useGitStore } from "@/state/gitStore"
import { useWorkspaceStore } from "@/state/workspaceStore"
import { workspacePathBasename, workspacePathForDisplay } from "@/lib/paths"
import { isMacPlatform, isWindowsPlatform } from "@/lib/platform"
import type { ContextMenuRequest } from "./contextMenuModel"
import { commandFor } from "./contextMenuDefs"
import { WorkspaceHostBadge } from "@/workbench/WorkspaceHostBadge"
import { FilesNavContent } from "./FilesNavContent"
import { GitNavContent } from "./GitNavContent"

export type WorkspaceTool = "files" | "git"

export function WorkspaceToolsPanel({ tool, onToolChange }: {
  tool: WorkspaceTool
  onToolChange: (tool: WorkspaceTool) => void
}) {
  const { t } = useTranslation("workbenchShell")
  const path = useWorkspaceStore(state => state.workspacePath)
  const [search, setSearch] = useState({ workspace: path, files: "", git: "" })
  // Reset only search state, keeping both mounted tool panes and their UI state.
  const queries = search.workspace === path ? search : { workspace: path, files: "", git: "" }
  if (search.workspace !== path) setSearch(queries)
  const displayPath = path ? workspacePathForDisplay(path) : t("noWorkspace")
  const title = path ? workspacePathBasename(path) : displayPath
  const directoryRequest: ContextMenuRequest | null = path
    ? { kind: "file", workspacePath: path, path, isDirectory: true, sourceGroupIndex: 0 }
    : null
  const copyCommand = directoryRequest ? commandFor(directoryRequest, "cmCopyFullPath") : null
  const revealCommand = directoryRequest ? commandFor(directoryRequest, "cmReveal") : null
  const revealLabel = t(isMacPlatform() ? "revealFinder" : isWindowsPlatform() ? "revealExplorer" : "revealFileManager")
  const create = (action: "cmNewFile" | "cmNewFolder") => {
    if (path) void executeLegacyContextMenuAction({ kind: "explorer", workspacePath: path }, action)
  }
  const refresh = () => {
    if (!path) return
    if (tool === "files") useWorkspaceStore.getState().refreshTree()
    else void Promise.all([useGitStore.getState().refresh(), useGitStore.getState().loadBranches()])
  }

  return <Card className="workbench-tools-card" size="sm">
    <CardHeader className="workbench-tools-card-header">
      <div className="workbench-tools-title-row">
        <CardTitle className="workbench-tools-title truncate" title={displayPath} data-tauri-drag-region>{title}</CardTitle>
        <div className="workbench-tools-actions">
          <Button variant="ghost" size="icon-sm" disabled={!path} title={t("newFile")} aria-label={t("newFile")} onClick={() => create("cmNewFile")}><FilePlus2 data-icon="inline-start" aria-hidden="true" /></Button>
          <Button variant="ghost" size="icon-sm" disabled={!path} title={t("newFolder")} aria-label={t("newFolder")} onClick={() => create("cmNewFolder")}><FolderPlus data-icon="inline-start" aria-hidden="true" /></Button>
          <Button variant="ghost" size="icon-sm" disabled={!path} title={t("refresh")} aria-label={t("refresh")} onClick={refresh}><RefreshCw data-icon="inline-start" aria-hidden="true" /></Button>
          <DropdownMenu>
            <DropdownMenuTrigger asChild><Button variant="ghost" size="icon-sm" title={t("moreTools")} aria-label={t("moreTools")}><MoreHorizontal data-icon="inline-start" aria-hidden="true" /></Button></DropdownMenuTrigger>
            <DropdownMenuContent align="end"><DropdownMenuGroup>
              <DropdownMenuItem disabled={!directoryRequest || !copyCommand?.availability(directoryRequest).enabled}
                onSelect={() => { if (directoryRequest && copyCommand) void runContextMenuAction(directoryRequest, copyCommand) }}>
                <Copy aria-hidden="true" />{t("copyWorkspacePath")}
              </DropdownMenuItem>
              <DropdownMenuItem disabled={!directoryRequest || !revealCommand?.availability(directoryRequest).enabled}
                onSelect={() => { if (directoryRequest && revealCommand) void runContextMenuAction(directoryRequest, revealCommand) }}>
                <FolderOpen aria-hidden="true" />{revealLabel}
              </DropdownMenuItem>
            </DropdownMenuGroup></DropdownMenuContent>
          </DropdownMenu>
        </div>
      </div>
      {path && <div className="workbench-tool-context"><WorkspaceHostBadge path={path} /></div>}
    </CardHeader>
    <CardContent className="workbench-tools-card-content">
      <InputGroup className="workbench-tools-search">
        <InputGroupAddon><ListFilter aria-hidden="true" /></InputGroupAddon>
        <InputGroupInput disabled={!path} aria-label={t("findFiles")} placeholder={t("findFiles")} value={queries[tool]}
          onChange={event => setSearch({ ...queries, [tool]: event.target.value })} />
      </InputGroup>
      <Tabs value={tool} onValueChange={value => { if (value === "files" || value === "git") onToolChange(value) }} className="workbench-tool-tabs">
        <TabsList aria-label={t("tools")} className="workbench-tool-switcher">
          <TabsTrigger value="files">{t("files")}</TabsTrigger>
          <TabsTrigger value="git">GIT</TabsTrigger>
        </TabsList>
        <TabsContent value="files" forceMount hidden={tool !== "files"} inert={tool !== "files"} className="workbench-tool-body"><FilesNavContent filterQuery={queries.files} active={tool === "files"} /></TabsContent>
        <TabsContent value="git" forceMount hidden={tool !== "git"} inert={tool !== "git"} className="workbench-tool-body"><GitNavContent filterQuery={queries.git} /></TabsContent>
      </Tabs>
    </CardContent>
  </Card>
}

import { Files, GitBranch, GitGraph } from "lucide-react"
import { useTranslation } from "react-i18next"
import { Button } from "@/components/ui/button"
import { Tabs, TabsContent, TabsList, TabsTrigger } from "@/components/ui/tabs"
import { useWorkspaceStore } from "@/state/workspaceStore"
import { workspacePathForDisplay } from "@/lib/paths"
import { WorkspaceHostBadge } from "@/workbench/WorkspaceHostBadge"
import { FilesNavContent } from "./FilesNavContent"
import { GitNavContent } from "./GitNavContent"

export type WorkspaceTool = "files" | "git"

export function WorkspaceToolsPanel({ tool, onToolChange, onOpenGraph }: {
  tool: WorkspaceTool
  onToolChange: (tool: WorkspaceTool) => void
  onOpenGraph: () => void
}) {
  const { t } = useTranslation("workbenchShell")
  const path = useWorkspaceStore(state => state.workspacePath)
  const displayPath = path ? workspacePathForDisplay(path) : t("noWorkspace")
  return <Tabs value={tool} onValueChange={value => { if (value === "files" || value === "git") onToolChange(value) }} className="workbench-tool-tabs">
      <TabsList aria-label={t("tools")} className="workbench-tool-switcher">
        <TabsTrigger value="files"><Files />{t("files")}</TabsTrigger>
        <TabsTrigger value="git"><GitBranch />Git</TabsTrigger>
      </TabsList>
      <div className="workbench-tool-context">
        <span className="truncate" title={displayPath}>{displayPath}</span>
        {path && <WorkspaceHostBadge path={path} />}
      </div>
      {tool === "git" && <Button variant="ghost" size="sm" className="workbench-graph-entry" onClick={onOpenGraph}><GitGraph />{t("gitGraph")}</Button>}
      <TabsContent value="files" forceMount hidden={tool !== "files"} inert={tool !== "files"} className="workbench-tool-body"><FilesNavContent /></TabsContent>
      <TabsContent value="git" forceMount hidden={tool !== "git"} inert={tool !== "git"} className="workbench-tool-body"><GitNavContent /></TabsContent>
    </Tabs>
}

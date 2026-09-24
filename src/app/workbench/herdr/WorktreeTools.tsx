import { useEffect, useState } from "react"
import { useTranslation } from "react-i18next"
import { FolderOpen, GitBranch, Plus, Trash2 } from "lucide-react"
import { Button } from "@/components/ui/button"
import { Badge } from "@/components/ui/badge"
import { Item, ItemActions, ItemContent, ItemDescription, ItemGroup, ItemMedia, ItemTitle } from "@/components/ui/item"
import { herdrWorktreeList } from "@/lib/herdrIpc"
import type { HerdrWorktreeListResult } from "@/lib/herdrTypes"
import { requestAppConfirmation } from "@/state/appDialogStore"
import { TextField, ToolStep } from "./controls"
import type { HerdrOperation } from "./useHerdrOperation"

export function WorktreeTools({ sessionName, workspaceId, operation, can }: {
  sessionName: string; workspaceId: string; operation: HerdrOperation; can: (method: string) => boolean
}) {
  const { t } = useTranslation("herdrTools")
  const [branch, setBranch] = useState("")
  const [base, setBase] = useState("")
  const [path, setPath] = useState("")
  const [inventory, setInventory] = useState<HerdrWorktreeListResult | null>(null)
  const [error, setError] = useState<string | null>(null)
  const [revision, setRevision] = useState(0)
  const listAvailable = can("worktree.list")
  useEffect(() => {
    let active = true
    if (workspaceId && listAvailable) void herdrWorktreeList({ sessionName, workspaceId }).then(value => { if (active) { setInventory(value); setError(null) } }).catch(cause => { if (active) setError(String(cause)) })
    return () => { active = false }
  }, [sessionName, workspaceId, revision, listAvailable])
  async function remove(id: string, force: boolean) {
    if (!await requestAppConfirmation({ title: t("removeWorktree"), description: t(force ? "forceWorktreeWarning" : "removeWorktreeWarning"), destructive: true, confirmLabel: t(force ? "forceRemove" : "remove") })) return
    if (await operation.run({ method: "worktree.remove", params: { workspace_id: id, force } })) setRevision(v => v + 1)
  }
  const worktrees = inventory?.worktrees ?? []
  return <div className="flex min-w-0 flex-col gap-6">
    <ToolStep index={1} title={t("createWorktree")}>
      <div className="flex min-w-0 flex-col gap-3">
        <p className="text-xs text-muted-foreground">{t("worktreeHint")}</p>
        <div className="grid min-w-0 gap-3 sm:grid-cols-2">
          <TextField label={t("branch")} value={branch} onChange={setBranch} placeholder="feature/new-idea" disabled={operation.busy} />
          <TextField label={t("base")} value={base} onChange={setBase} placeholder="HEAD" disabled={operation.busy} />
        </div>
        <TextField label={t("pathOptional")} hint={t("pathHint")} value={path} onChange={setPath} disabled={operation.busy} />
        <div className="flex justify-end">
          <Button disabled={operation.busy || !workspaceId || !can("worktree.create")} onClick={async () => {
            const result = await operation.run({ method: "worktree.create", params: { workspace_id: workspaceId, branch: branch || undefined, base: base || undefined, path: path || undefined, focus: false } })
            if (result) setRevision(v => v + 1)
          }}><Plus data-icon="inline-start" />{t("createWorktree")}</Button>
        </div>
      </div>
    </ToolStep>
    <ToolStep index={2} title={t("existingWorktrees")} aside={inventory && <Badge variant="secondary">{worktrees.length}</Badge>}>
      {error && <p role="alert" className="text-xs text-destructive">{error}</p>}
      {!error && !worktrees.length && <p className="text-xs text-muted-foreground">{t(listAvailable ? "noWorktrees" : "unavailable")}</p>}
      {worktrees.length > 0 && <ItemGroup className="gap-2">
        {worktrees.map(item => <Item key={item.path} variant="outline" size="sm" className="flex-nowrap">
          <ItemMedia variant="icon"><GitBranch /></ItemMedia>
          <ItemContent className="min-w-0">
            <ItemTitle className="flex min-w-0 max-w-full flex-wrap items-center gap-1.5">
              <span className="min-w-0 truncate">{item.branch ?? item.label}</span>
              {!item.isLinkedWorktree && <Badge variant="outline">{t("mainCheckout")}</Badge>}
              {item.openWorkspaceId && <Badge variant="secondary">{t("opened")}</Badge>}
            </ItemTitle>
            <ItemDescription className="truncate font-mono text-xs" title={item.path}>{item.path}</ItemDescription>
          </ItemContent>
          <ItemActions className="shrink-0">
            {!item.openWorkspaceId && <Button variant="outline" size="sm" disabled={operation.busy || !can("worktree.open")} onClick={async () => { if (await operation.run({ method: "worktree.open", params: { workspace_id: workspaceId, path: item.path, focus: false } })) setRevision(v => v + 1) }}><FolderOpen data-icon="inline-start" />{t("open")}</Button>}
            {item.isLinkedWorktree && item.openWorkspaceId && <>
              <Button variant="outline" size="sm" disabled={operation.busy || !can("worktree.remove")} onClick={() => void remove(item.openWorkspaceId!, false)}><Trash2 data-icon="inline-start" />{t("remove")}</Button>
              <Button variant="ghost" size="sm" className="text-destructive hover:text-destructive" disabled={operation.busy || !can("worktree.remove")} onClick={() => void remove(item.openWorkspaceId!, true)}>{t("forceRemove")}</Button>
            </>}
          </ItemActions>
        </Item>)}
      </ItemGroup>}
    </ToolStep>
  </div>
}

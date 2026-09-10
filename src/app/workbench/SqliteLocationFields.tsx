import { useEffect, useRef, useState } from "react"
import { useTranslation } from "react-i18next"
import { Folder, File } from "lucide-react"
import { Button } from "@/components/ui/button"
import { Input } from "@/components/ui/input"
import { Field, FieldDescription, FieldGroup, FieldLabel } from "@/components/ui/field"
import { Dialog, DialogContent, DialogDescription, DialogHeader, DialogTitle } from "@/components/ui/dialog"
import { Select, SelectContent, SelectGroup, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select"
import { ScrollArea } from "@/components/ui/scroll-area"
import { listRemoteDir, registerRuntimeWorkspace } from "@/lib/remoteFiles"
import { parseRemoteFilePath, remoteFilePath } from "@/lib/runtimeIdentity"
import type { DbSqliteWorkspace, FileNode } from "@/lib/types"
import { useHostStore } from "@/state/hostStore"

export function SqliteLocationFields({ path, onPathChange, workspace, onWorkspaceChange, disabled, onBrowseLocal }: {
  path: string; onPathChange: (path: string) => void
  workspace?: DbSqliteWorkspace; onWorkspaceChange: (workspace: DbSqliteWorkspace | undefined) => void
  disabled: boolean; onBrowseLocal: () => void
}) {
  const { t } = useTranslation("workbench")
  const hosts = useHostStore((state) => state.configs)
  const [browsing, setBrowsing] = useState(false)
  return <FieldGroup>
    <Field>
      <FieldLabel htmlFor="sqlite-source">{t("database.sqliteSource")}</FieldLabel>
      <Select value={workspace ? `host:${workspace.hostId}` : "local"} disabled={disabled} onValueChange={(value) => {
        onWorkspaceChange(value === "local" ? undefined : { hostId: value.slice(5), canonicalPath: "" })
        onPathChange("")
      }}>
        <SelectTrigger id="sqlite-source"><SelectValue /></SelectTrigger>
        <SelectContent><SelectGroup>
          <SelectItem value="local">{t("database.viaHostDirect")}</SelectItem>
          {Object.values(hosts).map((host) => <SelectItem key={host.hostId} value={`host:${host.hostId}`}>{host.label}</SelectItem>)}
          {workspace && !hosts[workspace.hostId] && <SelectItem value={`host:${workspace.hostId}`} disabled>{t("database.viaHostMissing", { host: workspace.hostId })}</SelectItem>}
        </SelectGroup></SelectContent>
      </Select>
    </Field>
    {workspace && <Field>
      <FieldLabel htmlFor="sqlite-workspace">{t("database.sqliteWorkspace")}</FieldLabel>
      <Input id="sqlite-workspace" value={workspace.canonicalPath} disabled={disabled} placeholder="/home/user/project" onChange={(event) => onWorkspaceChange({ ...workspace, canonicalPath: event.target.value })} />
      <FieldDescription>{t("database.sqliteRemoteDescription")}</FieldDescription>
    </Field>}
    <Field>
      <FieldLabel htmlFor="database-file-path">{t("database.fieldFile")}</FieldLabel>
      <div className="flex gap-2">
        <Input id="database-file-path" value={path} disabled={disabled} onChange={(event) => onPathChange(event.target.value)} placeholder={t("database.filePlaceholder")} className="flex-1" />
        <Button type="button" variant="outline" disabled={disabled || (!!workspace && !workspace.canonicalPath.startsWith("/"))} onClick={() => workspace ? setBrowsing(true) : onBrowseLocal()}>{t("database.browse")}</Button>
      </div>
    </Field>
    {browsing && workspace && <RemoteSqliteBrowser key={JSON.stringify(workspace)} workspace={workspace} onClose={() => setBrowsing(false)} onChoose={(workspace, path) => { onWorkspaceChange(workspace); onPathChange(path); setBrowsing(false) }} />}
  </FieldGroup>
}

function RemoteSqliteBrowser({ workspace, onClose, onChoose }: {
  workspace: DbSqliteWorkspace; onClose: () => void; onChoose: (workspace: DbSqliteWorkspace, path: string) => void
}) {
  const { t } = useTranslation("workbench")
  const connection = useHostStore((state) => state.hosts[workspace.hostId]?.connection)
  const [cwd, setCwd] = useState<string | null>(null)
  const [rows, setRows] = useState<FileNode[]>([])
  const [busy, setBusy] = useState(true)
  const [error, setError] = useState(false)
  const generation = useRef(0)
  useEffect(() => {
    const current = ++generation.current
    setBusy(true); setError(false)
    void (async () => {
      if (!connection) throw new Error("disconnected")
      const uri = await registerRuntimeWorkspace(connection.owner, workspace.canonicalPath, () => generation.current === current && useHostStore.getState().hosts[workspace.hostId]?.connection === connection)
      const rows = await listRemoteDir(uri)
      if (generation.current !== current || useHostStore.getState().hosts[workspace.hostId]?.connection !== connection) return
      setCwd(uri); setRows(rows)
    })().catch(() => { if (generation.current === current) setError(true) }).finally(() => { if (generation.current === current) setBusy(false) })
    return () => { generation.current++ }
  }, [connection, workspace.hostId, workspace.canonicalPath])
  async function browse(uri: string) {
    const current = ++generation.current
    setBusy(true); setError(false)
    try {
      const rows = await listRemoteDir(uri)
      if (generation.current !== current || useHostStore.getState().hosts[workspace.hostId]?.connection !== connection) return
      setCwd(uri); setRows(rows)
    } catch { if (generation.current === current) setError(true) }
    finally { if (generation.current === current) setBusy(false) }
  }
  const location = cwd ? parseRemoteFilePath(cwd) : null
  return <Dialog open onOpenChange={(open) => { if (!open) onClose() }}>
    <DialogContent>
      <DialogHeader><DialogTitle>{t("database.chooseRemoteSqlite")}</DialogTitle><DialogDescription>{location?.path ?? workspace.canonicalPath}</DialogDescription></DialogHeader>
      {busy && <p role="status">{t("database.sqliteBrowsing")}</p>}
      {error && <p role="alert">{t("database.sqliteBrowseFailed")}</p>}
      <ScrollArea className="h-64"><div className="flex flex-col gap-1">
        {location?.workspaceRoot && location.path !== location.workspaceRoot && <Button variant="ghost" disabled={busy} onClick={() => void browse(remoteFilePath(location.hostId, location.path.slice(0, location.path.lastIndexOf("/")) || "/", location.workspaceRoot!))}>..</Button>}
        {rows.map((row) => <Button key={row.path} variant="ghost" className="justify-start" disabled={busy || !connection || error} onClick={() => {
          if (row.isDir) { void browse(row.path); return }
          if (useHostStore.getState().hosts[workspace.hostId]?.connection !== connection) return
          const source = parseRemoteFilePath(row.path)
          if (source?.workspaceRoot) onChoose({ hostId: source.hostId, canonicalPath: source.workspaceRoot }, source.path)
        }}>{row.isDir ? <Folder data-icon="inline-start" /> : <File data-icon="inline-start" />}{row.name}</Button>)}
      </div></ScrollArea>
    </DialogContent>
  </Dialog>
}

import { useEffect, useRef, useState } from "react"
import { useTranslation } from "react-i18next"
import { open } from "@tauri-apps/plugin-dialog"
import { Folder, FolderOpen } from "lucide-react"
import { Button } from "@/components/ui/button"
import { Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle } from "@/components/ui/dialog"
import { Field, FieldGroup, FieldLabel } from "@/components/ui/field"
import { Input } from "@/components/ui/input"
import { ScrollArea } from "@/components/ui/scroll-area"
import { Tabs, TabsList, TabsTrigger } from "@/components/ui/tabs"
import { Checkbox } from "@/components/ui/checkbox"
import { HostList } from "@/app/workbench/HostList"
import { useFolderPickerStore } from "@/state/folderPickerStore"
import { useSshStore } from "@/state/sshStore"
import { sftpListDir } from "@/lib/ipc"
import { registerRuntimeWorkspace, registerSftpWorkspace } from "@/lib/remoteFiles"
import type { FileNode, SftpListing, WorkspaceOpenResult } from "@/lib/types"
import { requestHost, wslDistributions, wslPath } from "@/lib/hostIpc"
import type { HostTarget, WslDistribution } from "@/lib/hostIpc"
import { isWindowsPlatform } from "@/lib/platform"
import { useHostStore } from "@/state/hostStore"
import { LOCAL_HOST_ID, parseRemoteFilePath } from "@/lib/runtimeIdentity"
import { loadRemoteWorkspaces } from "@/state/remoteWorkspaceRegistry"
import { useRecentWorkspacesStore } from "@/state/recentWorkspaces"
import { workspacePathForDisplay } from "@/lib/paths"
import { WorkspaceHostBadge } from "./WorkspaceHostBadge"

export function FolderPickerHost() {
  const visible = useFolderPickerStore((state) => state.open)
  return visible ? <FolderPickerDialog /> : null
}

function FolderPickerDialog() {
  const { t } = useTranslation("hosts")
  const finish = useFolderPickerStore((state) => state.finish)
  const hosts = useSshStore((state) => state.hosts)
  const sessions = useSshStore((state) => state.sessions)
  const activeHostId = useSshStore((state) => state.activeHostId)
  const [location, setLocation] = useState(useFolderPickerStore.getState().initialLocation ?? "local")
  const legacyWindowsPath = useFolderPickerStore((state) => state.legacyWindowsPath)
  const runtimeHostId = useFolderPickerStore((state) => state.runtimeHostId)
  const runtimeConfig = useHostStore((state) => runtimeHostId ? state.configs[runtimeHostId] : undefined)
  const [access, setAccess] = useState("sftp")
  const [path, setPath] = useState(".")
  const [listing, setListing] = useState<SftpListing | null>(null)
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const [recent, setRecent] = useState<{ hostId: string; path: string; uri: string } | null>(null)
  const browseGeneration = useRef(0)
  const session = activeHostId ? sessions[activeHostId] : null
  const windows = isWindowsPlatform()
  const nativePicker = location === "local" && !windows
  const runtimePicker = (location === "local" && windows) || (location === "remote" && access === "runtime")

  useEffect(() => {
    setListing(null); setPath(recent?.hostId === activeHostId ? recent.path : "."); setError(null); setBusy(false)
    return () => { browseGeneration.current++ }
  }, [activeHostId, session?.sessionId, recent, location, access])

  function chooseRecent(uri: string) {
    try {
      const resource = parseRemoteFilePath(uri)
      if (!resource) { finish?.(uri); return }
      const access = loadRemoteWorkspaces()[uri] ?? "runtime"
      const config = useHostStore.getState().configs[resource.hostId]
      const connected = access === "runtime"
        ? useHostStore.getState().hosts[resource.hostId]?.connection
        : sessions[resource.hostId]?.status === "connected"
      if (connected) { finish?.(uri); return }
      if (config?.kind === "wsl") {
        if (!windows) { setError(t("recentHostUnavailable")); return }
        setRecent({ ...resource, uri })
        setLocation("local")
        return
      }
      if (!hosts.some((host) => host.id === resource.hostId)) { setError(t("recentHostUnavailable")); return }
      setRecent({ ...resource, uri })
      setLocation("remote")
      setAccess(access)
      useSshStore.getState().setActiveHost(resource.hostId)
      // Reuse the same password/key and host-key flow as a host-list click.
      useSshStore.getState().beginConnect(resource.hostId)
    } catch (cause) { setError(String(cause)) }
  }

  async function browse(remotePath = path) {
    if (!session?.sessionId) return
    const sessionId = session.sessionId
    const generation = ++browseGeneration.current
    const current = () => generation === browseGeneration.current
      && useSshStore.getState().activeHostId === activeHostId
      && useSshStore.getState().sessions[activeHostId!]?.sessionId === sessionId
    setBusy(true); setError(null)
    try {
      const result = await sftpListDir(sessionId, remotePath)
      if (!current()) return
      setListing(result); setPath(result.cwd)
    } catch (cause) { if (current()) setError(String(cause)) }
    finally { if (current()) setBusy(false) }
  }

  async function choose() {
    setBusy(true); setError(null)
    try {
      if (location === "local") {
        const selected = await open({ directory: true, multiple: false })
        if (typeof selected === "string") finish?.(selected)
      } else if (activeHostId && listing) {
        const selected = await registerSftpWorkspace(activeHostId, listing.cwd)
        finish?.(selected)
      }
    } catch (cause) { setError(String(cause)) }
    finally { setBusy(false) }
  }

  if (runtimeHostId && runtimeHostId !== LOCAL_HOST_ID) {
    const runtimeSession = sessions[runtimeHostId]
    const target: HostTarget | null = runtimeConfig?.kind === "wsl" && runtimeConfig.distro
      ? { kind: "wsl", distro: runtimeConfig.distro }
      : runtimeSession?.status === "connected" && runtimeSession.sessionId
        ? { kind: "ssh", sessionId: runtimeSession.sessionId } : null
    return <Dialog open onOpenChange={(next) => { if (!next) finish?.(null) }}>
      <DialogContent>
        <DialogHeader><DialogTitle>{t("addFolder")}</DialogTitle><DialogDescription>{runtimeConfig?.label ?? runtimeHostId}</DialogDescription></DialogHeader>
        {target ? <RuntimeFolderPicker hostId={runtimeHostId} label={runtimeConfig?.label ?? runtimeHostId} target={target} onChoose={(path) => finish?.(path)} /> : <p role="alert">{t("runtimeDisconnected")}</p>}
        <DialogFooter><Button variant="outline" onClick={() => finish?.(null)}>{t("cancel")}</Button></DialogFooter>
      </DialogContent>
    </Dialog>
  }

  return <>
    <Dialog open onOpenChange={(next) => { if (!next) finish?.(null) }}>
      <DialogContent>
        <DialogHeader><DialogTitle>{t("addFolder")}</DialogTitle><DialogDescription>{t("description")}</DialogDescription></DialogHeader>
        {!runtimeHostId && <Tabs value={location} onValueChange={(value) => { if (value === "local" || value === "remote") setLocation(value) }}>
          <TabsList><TabsTrigger value="local">{t("local")}</TabsTrigger><TabsTrigger value="remote">{t("remote")}</TabsTrigger></TabsList>
        </Tabs>}
        {location === "local" ? windows ? <WslFolderPicker key={recent?.uri} initialHostId={recent?.hostId} initialPath={recent?.path} legacyWindowsPath={legacyWindowsPath} onChoose={(path) => finish?.(path)} /> : <p>{t("localDescription")}</p> : <FieldGroup>
          <HostList />
          {session?.status === "connecting" && <p role="status">{t("connecting")}</p>}
          {session?.error && <p role="alert">{session.error}</p>}
          {session?.status === "connected" && <Tabs value={access} onValueChange={setAccess}><TabsList><TabsTrigger value="sftp">{t("sftp")}</TabsTrigger><TabsTrigger value="runtime">{t("runtime")}</TabsTrigger></TabsList></Tabs>}
          {session?.status === "connected" && session.sessionId && activeHostId && access === "runtime" && <RuntimeFolderPicker key={`${activeHostId}:${recent?.uri ?? ""}`} initialPath={recent?.hostId === activeHostId ? recent.path : undefined} hostId={activeHostId} label={hosts.find((host) => host.id === activeHostId)?.name ?? activeHostId} target={{kind:"ssh",sessionId:session.sessionId}} onChoose={(path) => finish?.(path)} />}
          {session?.status === "connected" && access === "sftp" && <Field>
            <FieldLabel htmlFor="remote-folder-path">{t("folder")}</FieldLabel>
            <div className="flex gap-2"><Input id="remote-folder-path" value={path} onChange={(event) => setPath(event.target.value)} onKeyDown={(event) => { if (event.key === "Enter") void browse() }} /><Button variant="outline" disabled={busy} onClick={() => void browse()}>{t("browse")}</Button></div>
            {listing && <ScrollArea className="h-48"><div className="flex flex-col gap-1">
              {listing.cwd !== "/" && <Button variant="ghost" disabled={busy} onClick={() => void browse(`${listing.cwd}/..`)}>..</Button>}
              {listing.entries.filter((entry) => entry.isDir && !entry.isSymlink && entry.nameSafe).map((entry) => <Button key={entry.path} variant="ghost" className="justify-start" disabled={busy} onClick={() => void browse(entry.path)}><Folder data-icon="inline-start" />{entry.name}</Button>)}
            </div></ScrollArea>}
            <p>{t("sftpDescription")}</p>
          </Field>}
        </FieldGroup>}
        {error && <p role="alert">{error}</p>}
        {!runtimeHostId && <RecentWorkspaceFolders onChoose={chooseRecent} />}
        <DialogFooter><Button variant="outline" onClick={() => finish?.(null)}>{t("cancel")}</Button>{!runtimePicker && <Button disabled={busy || (!nativePicker && !listing)} onClick={() => void choose()}><FolderOpen data-icon="inline-start" />{t("openFolder")}</Button>}</DialogFooter>
      </DialogContent>
    </Dialog>
  </>
}

function RecentWorkspaceFolders({ onChoose }: { onChoose: (path: string) => void }) {
  const { t } = useTranslation("hosts")
  const recent = useRecentWorkspacesStore((state) => state.list)
  if (!recent.length) return null
  return <Field>
    <FieldLabel>{t("recentFolders")}</FieldLabel>
    <ScrollArea className="max-h-40"><div className="flex flex-col gap-1">
      {recent.slice(0, 10).map((path) => <Button key={path} variant="ghost" className="justify-start" onClick={() => onChoose(path)}>
        <Folder data-icon="inline-start" /><span className="min-w-0 flex-1 truncate">{workspacePathForDisplay(path)}</span><WorkspaceHostBadge path={path} />
      </Button>)}
    </div></ScrollArea>
  </Field>
}

function WslFolderPicker({onChoose,legacyWindowsPath,initialHostId,initialPath}:{onChoose:(path:string)=>void;legacyWindowsPath?:string;initialHostId?:string;initialPath?:string}) {
  const {t}=useTranslation("hosts")
  const [distros,setDistros]=useState<WslDistribution[]>([])
  const [selected,setSelected]=useState<WslDistribution|null>(null)
  const [error,setError]=useState<string|null>(null)
  useEffect(() => {
    let active=true
    void wslDistributions().then((rows) => {if (active) {setDistros(rows);if(initialHostId) {const distro=rows.find((row)=>row.hostId===initialHostId && row.version===2);if(distro)setSelected(distro);else setError(t("recentHostUnavailable"))}}}).catch((error) => {if (active) setError(String(error))})
    return () => {active=false}
  },[initialHostId,t])
  return <FieldGroup>
    <p>{t("wslDescription")}</p>
    {legacyWindowsPath && <p>{t("bindLegacyWindows", { path: legacyWindowsPath })}</p>}
    <ScrollArea className="max-h-40"><div className="flex flex-col gap-2">{distros.map((distro) => <Button key={distro.hostId} disabled={distro.version!==2} variant={selected?.hostId===distro.hostId ? "secondary":"outline"} onClick={() => setSelected(distro)}>{distro.name} · WSL{distro.version}</Button>)}</div></ScrollArea>
    {distros.length===0 && <p>{t("wslEmpty")}</p>}
    {error && <p role="alert">{error}</p>}
    {selected && <RuntimeFolderPicker key={selected.hostId} initialPath={selected.hostId===initialHostId ? initialPath : undefined} hostId={selected.hostId} label={selected.name} target={{kind:"wsl",distro:selected.name}} legacyWindowsPath={legacyWindowsPath} onChoose={onChoose} />}
  </FieldGroup>
}

function RuntimeFolderPicker({hostId,label,target,onChoose,legacyWindowsPath,initialPath}:{hostId:string;label:string;target:HostTarget;onChoose:(path:string)=>void;legacyWindowsPath?:string;initialPath?:string}) {
  const {t}=useTranslation("hosts")
  const host=useHostStore((state)=>state.hosts[hostId])
  const [path,setPath]=useState(initialPath ?? "")
  const [directory,setDirectory]=useState<string|null>(null)
  const [entries,setEntries]=useState<FileNode[]>([])
  const [busy,setBusy]=useState(false)
  const [error,setError]=useState<string|null>(null)
  const [managed,setManaged]=useState(false)
  const owner=host?.connection?.owner
  async function setup() {
    setBusy(true);setError(null);setDirectory(null);setEntries([])
    try {const connection=await useHostStore.getState().setup(hostId,label,target,managed);setPath(initialPath ?? connection.hello.home)}
    catch(error) {setError(String(error))}
    finally {setBusy(false)}
  }
  async function browse(selected=path || host?.connection?.hello.home) {
    if (!owner || !selected) return
    setBusy(true);setError(null)
    try {
      const opened=await requestHost<WorkspaceOpenResult>(owner,{method:"workspaceOpen",params:{path:selected}})
      try {
        const rows=await requestHost<FileNode[]>(owner,{method:"filesList",params:{workspace:opened.capabilityId,path:""}})
        if (useHostStore.getState().hosts[hostId]?.connection?.owner !== owner) return
        setEntries(rows);setDirectory(opened.canonicalPath);setPath(opened.canonicalPath)
      } finally {await requestHost(owner,{method:"workspaceClose",params:{workspace:opened.capabilityId}}).catch(()=>undefined)}
    } catch(error) {setError(String(error))}
    finally {setBusy(false)}
  }
  async function choose() {
    if (!owner || !directory) return
    setBusy(true);setError(null)
    try {onChoose(await registerRuntimeWorkspace(owner,directory, () => useHostStore.getState().hosts[hostId]?.connection?.owner === owner))}
    catch(error) {setError(String(error))}
    finally {setBusy(false)}
  }
  async function chooseWindowsFolder() {
    if (target.kind!=="wsl" || !owner) return
    setBusy(true);setError(null)
    try {
      const selected=legacyWindowsPath ?? await open({directory:true,multiple:false})
      if (typeof selected==="string") await browse(await wslPath(hostId,target.distro,selected))
    } catch(error) {setError(String(error))}
    finally {setBusy(false)}
  }
  return <FieldGroup>
    {!owner ? <>
      <p>{t("setupDescription")}</p>
      <Field orientation="horizontal"><Checkbox id={`managed-${hostId}`} checked={managed} onCheckedChange={(value)=>setManaged(value===true)} /><FieldLabel htmlFor={`managed-${hostId}`}>{t("managedRuntime")}</FieldLabel></Field>
      <Button disabled={busy || host?.connecting} onClick={()=>void setup()}>{busy ? t("settingUp"):t("setupHost")}</Button>
    </> : <>
      <Field><FieldLabel htmlFor="runtime-folder">{t("folder")}</FieldLabel><div className="flex gap-2"><Input id="runtime-folder" value={path || host?.connection?.hello.home || ""} onChange={(event)=>setPath(event.target.value)} /><Button variant="outline" disabled={busy} onClick={()=>void browse()}>{t("browse")}</Button></div></Field>
      {target.kind==="wsl" && <Button variant="outline" disabled={busy} onClick={()=>void chooseWindowsFolder()}>{t(legacyWindowsPath ? "bindWindowsFolder" : "windowsFolder")}</Button>}
      {directory && <ScrollArea className="h-48"><div className="flex flex-col gap-1">
        {directory!=="/" && <Button variant="ghost" disabled={busy} onClick={()=>void browse(`${directory}/..`)}>..</Button>}
        {entries.filter((entry)=>entry.isDir && entry.kind!=="symlink").map((entry)=><Button key={entry.path} variant="ghost" className="justify-start" disabled={busy} onClick={()=>void browse(`${directory.replace(/\/$/,"")}/${entry.name}`)}><Folder data-icon="inline-start" />{entry.name}</Button>)}
      </div></ScrollArea>}
      <Button disabled={busy || !directory} onClick={()=>void choose()}><FolderOpen data-icon="inline-start" />{t("openFolder")}</Button>
      <Button variant="outline" disabled={busy || host?.connecting} onClick={()=>void setup()}>{t("updateHostTools")}</Button>
    </>}
    {(error || host?.error) && <p role="alert">{error ?? host?.error}</p>}
  </FieldGroup>
}

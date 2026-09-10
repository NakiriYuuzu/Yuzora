import { useTranslation } from "react-i18next"
import { SftpPanel } from "@/app/panels/SftpPanel"
import { PasswordPromptDialog } from "@/app/workbench/HostList"
import { Button } from "@/components/ui/button"
import { Dialog, DialogContent, DialogDescription, DialogHeader, DialogTitle } from "@/components/ui/dialog"
import { Select, SelectContent, SelectGroup, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select"
import { useSftpStore } from "@/state/sftpStore"
import { useSshStore } from "@/state/sshStore"
import { pickRemoteWorkspace } from "@/lib/workspaceActions"

export function SftpHost() {
  const { t } = useTranslation("hosts")
  const open = useSftpStore((state) => state.panelOpen)
  const hosts = useSshStore((state) => state.hosts)
  const active = useSshStore((state) => state.activeHostId)
  return <Dialog open={open} onOpenChange={(next) => useSftpStore.getState().setPanelOpen(next)}>
    <DialogContent className="flex h-[min(85vh,720px)] flex-col sm:max-w-5xl">
      <DialogHeader><DialogTitle>{t("transfers")}</DialogTitle><DialogDescription>{t("sftpDescription")}</DialogDescription></DialogHeader>
      <div className="flex gap-2">
        <Select value={active ?? ""} onValueChange={(id) => useSshStore.getState().beginConnect(id)}>
          <SelectTrigger aria-label={t("hosts")}><SelectValue placeholder={t("hosts")} /></SelectTrigger>
          <SelectContent><SelectGroup>{hosts.map((host) => <SelectItem key={host.id} value={host.id}>{host.name} · {host.user}@{host.host}</SelectItem>)}</SelectGroup></SelectContent>
        </Select>
        <Button variant="outline" onClick={() => { useSftpStore.getState().setPanelOpen(false); void pickRemoteWorkspace() }}>{t("manageHosts")}</Button>
      </div>
      <SftpPanel />
    </DialogContent>
  </Dialog>
}

/** One auth prompt across the folder picker, runtime, database and transfer surfaces. */
export function SshAuthenticationHost() {
  const pending = useSshStore((state) => state.pendingAuthHostId)
  const host = useSshStore((state) => state.hosts.find((host) => host.id === pending))
  return host ? <PasswordPromptDialog key={host.id} host={host} /> : null
}

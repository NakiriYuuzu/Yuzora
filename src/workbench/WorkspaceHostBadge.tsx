import { useTranslation } from "react-i18next"
import { Badge } from "@/components/ui/badge"
import { LOCAL_HOST_ID, parseRemoteFilePath } from "@/lib/runtimeIdentity"
import { useHostStore } from "@/state/hostStore"
import { useSshStore } from "@/state/sshStore"

/** Display names never participate in workspace identity or I/O routing. */
export function WorkspaceHostBadge({ path, hostId }: { path?: string; hostId?: string }) {
  const { t } = useTranslation("hosts")
  const id = hostId ?? (path ? parseRemoteFilePath(path)?.hostId : null) ?? LOCAL_HOST_ID
  const sshName = useSshStore((state) => state.hosts.find((host) => host.id === id)?.name)
  const runtimeName = useHostStore((state) => state.configs[id]?.label)
  const label = id === LOCAL_HOST_ID ? t("local") : sshName ?? runtimeName ?? id
  return <Badge variant="outline" title={id}><span className="max-w-28 truncate">{label}</span></Badge>
}

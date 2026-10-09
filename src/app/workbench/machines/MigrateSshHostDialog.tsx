import { useTranslation } from "react-i18next"
import { buildMachineTarget } from "@/lib/machinesTarget"
import type { SshHost } from "@/state/sshStore"
import { MachineAddDialog, type MachineAddValues } from "./MachineAddDialog"

/** Prefills the add form from a saved SSH host. The saved host and sshStore stay untouched. */
export function MigrateSshHostDialog({ host, onSubmit, onCancel }: {
  host: SshHost
  onSubmit: (values: MachineAddValues) => void
  onCancel: () => void
}) {
  const { t } = useTranslation("machines")
  return <MachineAddDialog
    title={t("migrate.title", { name: host.name })}
    description={t("migrate.description")}
    initial={{ target: buildMachineTarget(host), label: host.name }}
    notice={host.authKind === "key" || host.keyPath ? t("migrate.keyWarning") : undefined}
    onSubmit={onSubmit}
    onCancel={onCancel}
  />
}

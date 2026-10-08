import { Fragment } from "react"
import { useTranslation } from "react-i18next"
import { Folder, Server, TriangleAlert } from "lucide-react"
import { Button } from "@/components/ui/button"
import { parseMachineError } from "@/lib/machinesErrors"
import type { HerdrMachine, HerdrMachineAgent } from "@/lib/machinesTypes"
import { useMachinesInteractiveStore } from "@/state/machinesInteractiveStore"
import { useMachinesStore } from "@/state/machinesStore"
import { AgentLogo } from "../AgentLogo"
import { resolveAgentKind } from "../agentLogos"

const machineNodeKey = (machineId: string) => JSON.stringify(["machine", machineId])
const machineAgentNodeKey = (machineId: string, terminalId: string) => JSON.stringify(["machine", machineId, "agent", terminalId])

const machineAgentTitle = (agent: HerdrMachineAgent) => agent.name ?? agent.title ?? agent.agent ?? agent.terminalId

/** Agents view only: enabled HERDR machines and the agents their last snapshot reported. */
export function MachineAgentGroup() {
  const { t } = useTranslation("machines")
  const { t: ts } = useTranslation("spaceTree")
  const machines = useMachinesStore((state) => state.machines)
  const snapshotById = useMachinesStore((state) => state.snapshotById)
  const staleById = useMachinesStore((state) => state.staleById)
  const errorById = useMachinesStore((state) => state.errorById)
  const enabled = machines.filter((machine) => machine.enabled)
  if (!enabled.length) return null
  const openClient = (machine: HerdrMachine) => {
    useMachinesInteractiveStore.getState().open({ spec: { kind: "client" }, machineLabel: machine.label })
  }
  return <Fragment>
    <div className="tree-session-heading" data-machines-group>
      <strong>{t("sidebar.group")}</strong>
    </div>
    {enabled.map((machine) => {
      const snapshot = snapshotById[machine.id]
      const error = errorById[machine.id]
      const stale = Boolean(staleById[machine.id])
      const code = error ? parseMachineError(error).code : null
      const health = code === "machines-auth-required" ? "auth-required" : error ? "error" : snapshot ? "reachable" : "unknown"
      const note = health === "auth-required" ? t("sidebar.authRequired") : stale ? t("sidebar.stale") : error ? t("sidebar.error") : null
      const hint = t("sidebar.openClient", { label: machine.label })
      return <Fragment key={machine.id}>
        <div role="none" className="tree-row-shell tree-row-machine">
          <Button
            variant="ghost"
            role="treeitem"
            tabIndex={0}
            aria-level={1}
            aria-selected={false}
            aria-label={`${machine.label}${note ? ` · ${note}` : ""}`}
            title={hint}
            data-node-key={machineNodeKey(machine.id)}
            className="space-tree-row tree-machine"
            onClick={() => openClient(machine)}
          >
            <Server aria-hidden="true" />
            <span className="tree-node-label"><span title={machine.label}>{machine.label}</span></span>
            {note && <span className="tree-machine-note" data-health={health}><TriangleAlert aria-hidden="true" />{note}</span>}
            <span className="tree-status-dot" data-machine-health={health} />
          </Button>
        </div>
        {snapshot && !snapshot.agents.length && <p className="tree-machine-empty">{t("sidebar.noAgents")}</p>}
        {snapshot?.agents.map((agent) => {
          const title = machineAgentTitle(agent)
          return <div key={agent.terminalId} role="none" className="tree-row-shell tree-row-machineAgent">
            <Button
              variant="ghost"
              role="treeitem"
              tabIndex={0}
              aria-level={2}
              aria-selected={false}
              aria-label={`${title} · ${ts(`status.${agent.status}`)} · ${machine.label}`}
              title={hint}
              data-node-key={machineAgentNodeKey(machine.id, agent.terminalId)}
              className="space-tree-row tree-machineAgent tree-agent-tagged"
              style={{ paddingLeft: 16 }}
              onClick={() => openClient(machine)}
            >
              <span className="tree-agent-avatar" aria-hidden="true">
                <AgentLogo kind={resolveAgentKind(agent.agent, agent.name, title)} label={agent.name ?? title} />
                <span className="tree-status-dot" data-status={agent.status} />
              </span>
              <span className="tree-node-label">
                <span>{title}</span>
                <span className="tree-agent-tags">
                  <span className="tree-agent-tag" data-tag="machine" title={machine.label}>
                    <Server aria-hidden="true" />
                    <span>{machine.label}</span>
                  </span>
                  {agent.folder && <span className="tree-agent-tag" data-tag="folder" title={agent.cwd ?? agent.folder}>
                    <Folder aria-hidden="true" />
                    <span>{agent.folder}</span>
                  </span>}
                </span>
              </span>
              <span className="tree-agent-status" data-status={agent.status}>{ts(`status.${agent.status}`)}</span>
            </Button>
          </div>
        })}
      </Fragment>
    })}
  </Fragment>
}

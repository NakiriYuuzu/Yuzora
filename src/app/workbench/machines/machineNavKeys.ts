import type { HerdrMachine, HerdrMachineSnapshot } from "@/lib/machinesTypes"

export const machineNodeKey = (machineId: string) => JSON.stringify(["machine", machineId])
export const machineAgentNodeKey = (machineId: string, terminalId: string) => JSON.stringify(["machine", machineId, "agent", terminalId])

/** Keyboard order of the machine rows (group row, then its agents) for the tree's roving tabindex. */
export function machineNavKeys(machines: HerdrMachine[], snapshotById: Record<string, HerdrMachineSnapshot>): string[] {
  return machines.filter((machine) => machine.enabled).flatMap((machine) => [
    machineNodeKey(machine.id),
    ...(snapshotById[machine.id]?.agents ?? []).map((agent) => machineAgentNodeKey(machine.id, agent.terminalId))
  ])
}


import { Channel } from "@tauri-apps/api/core"
import { invoke } from "@/lib/ipc"
import type { HerdrTerminalEvent } from "@/lib/herdrTypes"
import type {
  HerdrMachine,
  HerdrMachineInteractiveOpened,
  HerdrMachineInteractiveSpec,
  HerdrMachineSnapshot,
  HerdrMachineStatus,
  HerdrMachinesCapabilities
} from "@/lib/machinesTypes"

// Machines commands run on the local manager only: no sessionName, no runtime-scope routing.
export const machinesCapabilities = () => invoke<HerdrMachinesCapabilities>("herdr_machines_capabilities")
export const machinesList = () => invoke<HerdrMachine[]>("herdr_machines_list")
export const machinesStatus = (machineId: string) => invoke<HerdrMachineStatus>("herdr_machines_status", { machineId })
export const machinesAgents = (machineId: string) => invoke<HerdrMachineSnapshot>("herdr_machines_agents", { machineId })
export const machinesRename = (machineId: string, label: string) => invoke<HerdrMachine[]>("herdr_machines_rename", { machineId, label })
export const machinesSetEnabled = (machineId: string, enabled: boolean) => invoke<HerdrMachine[]>("herdr_machines_set_enabled", { machineId, enabled })
export const machinesRemove = (machineId: string) => invoke<HerdrMachine[]>("herdr_machines_remove", { machineId })

export interface MachinesClientSize {
  cols: number
  rows: number
  cellWidth: number
  cellHeight: number
}

export function machinesInteractiveOpen(
  spec: HerdrMachineInteractiveSpec,
  size: MachinesClientSize,
  onEvent: (event: HerdrTerminalEvent) => void
) {
  return invoke<HerdrMachineInteractiveOpened>("herdr_machine_interactive_open", {
    spec,
    size,
    onEvent: new Channel(onEvent)
  })
}

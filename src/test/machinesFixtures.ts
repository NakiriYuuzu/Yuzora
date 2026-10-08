import type { HerdrMachine, HerdrMachineSnapshot, HerdrMachinesCapabilities } from "@/lib/machinesTypes"

export const machine = (id: string, patch: Partial<HerdrMachine> = {}): HerdrMachine => ({
  id: id.padEnd(32, "0"), label: `Machine ${id}`, target: `${id}@host`, session: "default", enabled: true, selected: false, ...patch
})
export const supportedCaps: HerdrMachinesCapabilities = {
  binaryPath: "/herdr", version: "0.9.3", supported: true, hasStatus: true, hasReconnect: true, source: "default", reason: null
}
export const snapshot = (machineId: string, agents: HerdrMachineSnapshot["agents"] = []): HerdrMachineSnapshot => ({
  machineId, fetchedAt: 1, serverVersion: "0.9.3", workspaces: [], agents
})

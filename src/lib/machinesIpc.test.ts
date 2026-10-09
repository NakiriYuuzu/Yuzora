import { Channel } from "@tauri-apps/api/core"
import { clearMocks, mockIPC } from "@tauri-apps/api/mocks"
import { afterEach, describe, expect, it, vi } from "vitest"
import {
  machinesAgents, machinesCapabilities, machinesInteractiveOpen, machinesList, machinesRemove,
  machinesRename, machinesSetEnabled, machinesStatus
} from "./machinesIpc"

const calls: Array<{ cmd: string; payload: Record<string, unknown> | undefined }> = []
function setup() {
  calls.length = 0
  mockIPC((cmd, payload) => { calls.push({ cmd, payload: payload as Record<string, unknown> | undefined }); return [] })
}
afterEach(() => clearMocks())
const ID = "0123456789abcdef0123456789abcdef"

describe("machines IPC wrappers", () => {
  it("sends exact command names and payloads", async () => {
    setup()
    await machinesCapabilities()
    await machinesList()
    await machinesStatus(ID)
    await machinesAgents(ID)
    await machinesRename(ID, "Lab")
    await machinesSetEnabled(ID, false)
    await machinesRemove(ID)
    expect(calls.map(call => call.cmd)).toEqual([
      "herdr_machines_capabilities", "herdr_machines_list", "herdr_machines_status", "herdr_machines_agents",
      "herdr_machines_rename", "herdr_machines_set_enabled", "herdr_machines_remove"
    ])
    expect(calls.map(call => call.payload ?? {})).toEqual([
      {}, {}, { machineId: ID }, { machineId: ID }, { machineId: ID, label: "Lab" },
      { machineId: ID, enabled: false }, { machineId: ID }
    ])
  })

  it("never routes by session name", async () => {
    setup()
    await machinesStatus(ID)
    expect(Object.keys(calls[0].payload ?? {})).toEqual(["machineId"])
  })

  it("opens the interactive client with spec, size and a Channel", async () => {
    setup()
    const spec = { kind: "add", target: "me@host", label: "Lab" } as const
    const size = { cols: 80, rows: 24, cellWidth: 8, cellHeight: 16 }
    await machinesInteractiveOpen(spec, size, vi.fn())
    expect(calls[0].cmd).toBe("herdr_machine_interactive_open")
    expect(Object.keys(calls[0].payload!).sort()).toEqual(["onEvent", "size", "spec"])
    expect(calls[0].payload!.spec).toEqual(spec)
    expect(calls[0].payload!.size).toEqual(size)
    expect(calls[0].payload!.onEvent).toBeInstanceOf(Channel)
  })
})

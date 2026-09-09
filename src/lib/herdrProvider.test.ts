import { afterEach, beforeEach, describe, expect, it } from "vitest"
import { clearMocks, mockIPC } from "@tauri-apps/api/mocks"
import type { ConnectedHost } from "./hostIpc"
import { findRuntimeSession, invokeHerdr, registerRuntimeHost, sessionScope, StaleRuntimeResponse, unregisterRuntimeHost } from "./herdrProvider"
import { remoteFilePath, runtimeKey } from "./runtimeIdentity"

const host = (hostId: string, generation = 1): ConnectedHost => ({
  owner: { hostId, generation },
  hello: { protocol: 1, version: "test", os: "linux", arch: "x86_64", home: "/home/me", methods: [] }
})
const session = { name: "same", default: true, running: true, sessionDir: "/same", socketPath: "/same/socket" }
beforeEach(() => {
  registerRuntimeHost(host("a"), "/herdr", "A")
  registerRuntimeHost(host("b"), "/herdr", "B")
})
afterEach(() => {
  for (const id of ["a", "b"]) for (const generation of [1, 2]) unregisterRuntimeHost(host(id, generation).owner)
  clearMocks()
})

describe("host runtime routing", () => {
  it("resolves same-name remote Sessions and keeps legacy live pages local", () => {
    const scope = runtimeKey({ hostId: "a", sessionName: "same" })
    const remote = { ...session, hostId: "a", runtimeId: scope }
    const local = { ...session, running: false }
    expect(findRuntimeSession([remote, local], scope)).toBe(remote)
    expect(findRuntimeSession([remote, local], "live")).toBe(local)
    expect(findRuntimeSession([remote], "live")).toBeNull()
    expect(findRuntimeSession([remote], "same")).toBeNull()
  })

  it("keeps same-name sessions separate and uses the real name on their host", async () => {
    const calls: Array<Record<string, unknown>> = []
    mockIPC((command, args) => {
      if (command === "herdr_sessions") return [session]
      calls.push(args as Record<string, unknown>)
      return ((args as Record<string, unknown>).operation as { params: { call: { command: string } } }).params.call.command === "herdr_sessions" ? [session] : { ok: true }
    })
    const sessions = await invokeHerdr<Array<typeof session>>("herdr_sessions")
    expect(new Set(sessions.map(sessionScope)).size).toBe(3)
    const scope = runtimeKey({ hostId: "b", sessionName: "same" })
    await invokeHerdr("herdr_workspace_create", { sessionName: scope, cwd: remoteFilePath("b", "/repo 中文"), focus: true })
    expect(calls.at(-1)).toMatchObject({
      owner: { hostId: "b", generation: 1 },
      operation: { method: "herdrCall", params: { call: { command: "herdr_workspace_create", args: { sessionName: "same", cwd: "/repo 中文" } } } }
    })
    await expect(invokeHerdr("herdr_workspace_create", { sessionName: scope, cwd: remoteFilePath("a", "/repo") })).rejects.toThrow("different runtime host")
  })

  it("discards an old generation's mutation response without replaying it", async () => {
    let resolve!: (value: unknown) => void
    let calls = 0
    mockIPC(() => { calls++; return new Promise((done) => { resolve = done }) })
    const pending = invokeHerdr("herdr_tab_close", { sessionName: runtimeKey({ hostId: "a", sessionName: "same" }), tabId: "tab-1" })
    registerRuntimeHost(host("a", 2), "/herdr", "A")
    resolve(null)
    await expect(pending).rejects.toBeInstanceOf(StaleRuntimeResponse)
    expect(calls).toBe(1)
  })

  it.each(["host-call-limit", "host-call-wait-timeout", "host-request-limit", "host-request-wait-timeout"])("keeps the last Session inventory when the host is busy: %s", async (error) => {
    let busy = false
    mockIPC((command) => {
      if (command === "herdr_sessions") return [session]
      if (busy) throw error
      return [session]
    })
    const initial = await invokeHerdr<Array<typeof session>>("herdr_sessions")
    busy = true
    expect(await invokeHerdr("herdr_sessions")).toEqual(initial)
  })

  it("does not route a disconnected remote session to a local command", async () => {
    let calls = 0
    mockIPC(() => { calls++; return null })
    unregisterRuntimeHost(host("a").owner)
    await expect(invokeHerdr("herdr_snapshot", { sessionName: runtimeKey({ hostId: "a", sessionName: "same" }) })).rejects.toThrow("disconnected")
    expect(calls).toBe(0)
  })
})

it("does not invoke WSL calls or streams while its preference is disabled", async () => {
  const { useRuntimePreferencesStore } = await import("@/state/runtimePreferencesStore")
  useRuntimePreferencesStore.setState({ wslEnabled: false })
  registerRuntimeHost(host("wsl-disabled"), "/wsl/herdr", "Ubuntu", "wsl")
  const calls: string[] = []
  mockIPC(command => { calls.push(command); return [session] })
  try {
    expect(await invokeHerdr("herdr_sessions")).not.toContainEqual(expect.objectContaining({ hostId: "wsl-disabled" }))
    expect(calls).toEqual(["herdr_sessions", "host_request", "host_request"])
    calls.length = 0
    const sessionName = runtimeKey({ hostId: "wsl-disabled", sessionName: "default" })
    await expect(invokeHerdr("herdr_snapshot", { sessionName })).rejects.toThrow("wsl-runtime-disabled")
    await expect(invokeHerdr("herdr_terminal_open", { sessionName })).rejects.toThrow("wsl-runtime-disabled")
    expect(calls).toEqual([])
  } finally { unregisterRuntimeHost(host("wsl-disabled").owner) }
})

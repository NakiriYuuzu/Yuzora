import { afterEach, expect, it, vi } from "vitest"
import { Channel } from "@tauri-apps/api/core"
import { clearMocks, mockIPC } from "@tauri-apps/api/mocks"
import { registerRuntimeHost, unregisterRuntimeHost } from "./herdrProvider"
import { herdrEventsRelease, herdrEventsSubscribe, herdrTerminalOpen, herdrTerminalRelease } from "./herdrIpc"

const callbacks = new Map<number, (message: unknown) => void>()
const channels = new Map<string, Channel<unknown>>()
let sequence = 0
const host = (generation = 1) => ({ owner: { hostId: "lifecycle", generation }, hello: { protocol: 1, version: "fixture", os: "linux", arch: "arm64", home: "/tmp", methods: [] } })
const scope = JSON.stringify(["lifecycle", "default"])
function setup() {
  mockIPC((command, payload) => {
    const args = payload as any
    if (command === "host_stream_open") {
      const streamId = String(++sequence)
      channels.set(streamId, args.onEvent)
      return { streamId, value: { target: "term", mode: "control", role: "controller", takeover: true } }
    }
    if (command === "host_stream_close") {
      end(args.streamId)
      return null
    }
    throw new Error(`Unexpected ${command}`)
  })
  const internals = (window as any).__TAURI_INTERNALS__
  vi.spyOn(internals, "transformCallback").mockImplementation((fn: any) => { const id = ++sequence; callbacks.set(id, fn); return id })
  internals.unregisterCallback = (id: number) => callbacks.delete(id)
  registerRuntimeHost(host(), "/herdr", "Fixture")
}
function end(id: string) {
  const channel = channels.get(id)
  if (channel) callbacks.get(channel.id)?.({ end: true, index: 0 })
  channels.delete(id)
}
afterEach(() => {
  unregisterRuntimeHost(host().owner)
  unregisterRuntimeHost(host(2).owner)
  channels.clear(); callbacks.clear(); vi.restoreAllMocks(); clearMocks()
})
it("returns callbacks to baseline after repeated remote terminal and subscription closes", async () => {
  setup()
  for (let i = 0; i < 100; i++) {
    const terminal = await herdrTerminalOpen({ target: "term", cols: 80, rows: 24, sessionName: scope, onEvent() {} })
    await herdrTerminalRelease(terminal.sessionId)
    const subscription = await herdrEventsSubscribe({ sessionName: scope, onEvent() {} })
    await herdrEventsRelease(subscription)
  }
  expect(channels.size).toBe(0)
  expect(callbacks.size).toBe(0)
})
it("allows idempotent release after host removal and ignores old closed events after reconnect", async () => {
  setup()
  const old = await herdrEventsSubscribe({ sessionName: scope, onEvent() {} })
  unregisterRuntimeHost(host().owner)
  registerRuntimeHost(host(2), "/herdr", "Fixture")
  const current = await herdrEventsSubscribe({ sessionName: scope, onEvent() {} })
  const oldId = JSON.parse(old)[2]
  channels.get(oldId)?.onmessage({ type: "closed", owner: host().owner, streamId: oldId, reason: "disconnected" })
  end(oldId)
  await expect(herdrEventsRelease(old)).resolves.toBeUndefined()
  expect(channels.size).toBe(1)
  await herdrEventsRelease(current)
  expect(callbacks.size).toBe(0)
})
it("does not allocate a Channel when the remote host is already disconnected", async () => {
  setup()
  unregisterRuntimeHost(host().owner)
  await expect(herdrEventsSubscribe({ sessionName: scope, onEvent() {} })).rejects.toThrow("disconnected")
  expect(callbacks.size).toBe(0)
})

import { afterEach, beforeEach, expect, it, vi } from "vitest"
vi.mock("@tauri-apps/api/core", () => ({ Channel: class { onmessage = (_value: unknown) => {} } }))
vi.mock("@tauri-apps/api/event", () => ({ emit: vi.fn(async () => {}) }))
vi.mock("./ipc", () => ({ invoke: vi.fn(), sftpListDir: vi.fn() }))
vi.mock("@/state/sshStore", () => ({ useSshStore: { getState: vi.fn() } }))
import { invoke } from "./ipc"
import { emit } from "@tauri-apps/api/event"
import { registerRuntimeWorkspace, startRemoteWatch, stopRemoteWatch } from "./remoteFiles"

let sequence = 0
beforeEach(() => {
  vi.useFakeTimers()
  vi.clearAllMocks()
  vi.mocked(invoke).mockReset().mockImplementation(async (command) => command === "host_request"
    ? { canonicalPath: "/project", capabilityId: "cap" }
    : { streamId: "watch" })
})
afterEach(async () => { await stopRemoteWatch(); vi.useRealTimers() })
const settle = async () => { for (let i = 0; i < 8; i++) await Promise.resolve() }
async function workspace() {
  const owner = { hostId: `files-watch-${++sequence}`, generation: 1 }
  return { owner, uri: await registerRuntimeWorkspace(owner, "/project", () => true) }
}
function channel(index = 0) {
  const args = vi.mocked(invoke).mock.calls.filter(([name]) => name === "host_stream_open")[index][1] as { onEvent: { onmessage: (message: unknown) => void } }
  return args.onEvent
}

it("releases a late opening when the active watcher was stopped", async () => {
  const { owner, uri } = await workspace()
  let finish!: (value: unknown) => void
  vi.mocked(invoke).mockImplementationOnce(() => new Promise((resolve) => { finish = resolve }))
  const opening = startRemoteWatch(uri)
  await settle()
  await stopRemoteWatch()
  finish({ streamId: "late" })
  await opening
  expect(invoke).toHaveBeenLastCalledWith("host_stream_close", { owner, streamId: "late" })
  expect(emit).not.toHaveBeenCalled()
})

it("resets the file snapshot after a watcher retry and cancels pending retries on close", async () => {
  const { owner, uri } = await workspace()
  await startRemoteWatch(uri)
  expect(emit).toHaveBeenCalledWith("fs:external-change", { workspaceRoot: uri, paths: [uri] })
  channel().onmessage({ type: "closed", owner })
  channel().onmessage({ type: "closed", owner })
  await vi.advanceTimersByTimeAsync(1000)
  expect(vi.mocked(invoke).mock.calls.filter(([name]) => name === "host_stream_open")).toHaveLength(2)
  expect(emit).toHaveBeenCalledTimes(2)
  channel(1).onmessage({ type: "closed", owner })
  await stopRemoteWatch()
  await vi.advanceTimersByTimeAsync(30000)
  expect(vi.mocked(invoke).mock.calls.filter(([name]) => name === "host_stream_open")).toHaveLength(2)
})

it("drops notifications from a backend replaced before watcher reattachment", async () => {
  const { owner, uri } = await workspace()
  await startRemoteWatch(uri)
  vi.mocked(emit).mockClear()
  await registerRuntimeWorkspace({ ...owner, generation: 2 }, "/project", () => true)
  channel().onmessage({ type: "frame", frame: { version: 1, owner, payload: { type: "files", workspaceRoot: "/project", paths: ["/project/stale.txt"] } } })
  expect(emit).not.toHaveBeenCalled()
  await vi.advanceTimersByTimeAsync(1000)
  expect(invoke).toHaveBeenCalledWith("host_stream_close", { owner, streamId: "watch" })
  expect(emit).toHaveBeenCalledExactlyOnceWith("fs:external-change", { workspaceRoot: uri, paths: [uri] })
})

it("closes a stream opened against a backend that was replaced during the await", async () => {
  const { owner, uri } = await workspace()
  let finish!: (value: unknown) => void
  vi.mocked(invoke).mockImplementationOnce(() => new Promise((resolve) => { finish = resolve }))
  const opening = startRemoteWatch(uri)
  await settle()
  await registerRuntimeWorkspace({ ...owner, generation: 2 }, "/project", () => true)
  finish({ streamId: "old" })
  await opening
  expect(invoke).toHaveBeenLastCalledWith("host_stream_close", { owner, streamId: "old" })
  expect(emit).not.toHaveBeenCalled()
})

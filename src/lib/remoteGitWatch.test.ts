import { afterEach, beforeEach, expect, it, vi } from "vitest"
vi.mock("@tauri-apps/api/core", () => ({ Channel: class { onmessage = (_value: unknown) => {} } }))
vi.mock("./ipc", () => ({ invoke: vi.fn() }))
vi.mock("./remoteFiles", () => ({ runtimeWorkspaceService: vi.fn() }))
vi.mock("./remoteTrust", () => ({ requestWorkspace: vi.fn() }))
import { invoke } from "./ipc"
import { runtimeWorkspaceService } from "./remoteFiles"
import { requestWorkspace } from "./remoteTrust"
import { watchRemoteGit } from "./remoteGitWatch"
import { remoteFilePath } from "./runtimeIdentity"

const owner = { hostId: "git-watch", generation: 3 }
const uri = remoteFilePath(owner.hostId, "/project")
const assertCurrent = vi.fn()
let dispose: (() => void) | undefined
beforeEach(() => {
  vi.useFakeTimers()
  vi.clearAllMocks()
  assertCurrent.mockReset()
  vi.mocked(requestWorkspace).mockResolvedValue(null)
  vi.mocked(invoke).mockReset().mockResolvedValue({ streamId: "git-watch-1" })
  vi.mocked(runtimeWorkspaceService).mockReturnValue({ uri, owner, root: "/project", capabilityId: "cap", assertCurrent })
})
afterEach(() => { dispose?.(); dispose = undefined; vi.useRealTimers() })
const settle = async () => { for (let i = 0; i < 8; i++) await Promise.resolve() }
function channel(index = 0) {
  const args = vi.mocked(invoke).mock.calls.filter(([name]) => name === "host_stream_open")[index][1] as { onEvent: { onmessage: (message: unknown) => void } }
  return args.onEvent
}
function event(eventOwner = owner, workspaceRoot = "/project") {
  return { type: "frame", frame: { version: 1, owner: eventOwner, payload: { type: "git", workspaceRoot } } }
}

it("invalidates after opening and accepts only the owning workspace generation", async () => {
  const refresh = vi.fn()
  dispose = watchRemoteGit(uri, uri, refresh)
  await settle()
  expect(refresh).toHaveBeenCalledOnce()
  channel().onmessage(event({ ...owner, hostId: "other" }))
  channel().onmessage(event({ ...owner, generation: 2 }))
  channel().onmessage(event(owner, "/other"))
  expect(refresh).toHaveBeenCalledOnce()
  channel().onmessage(event())
  expect(refresh).toHaveBeenCalledTimes(2)
  dispose()
  channel().onmessage(event())
  expect(refresh).toHaveBeenCalledTimes(2)
})

it("closes a late opening after the effect was disposed", async () => {
  let finish!: (value: unknown) => void
  vi.mocked(invoke).mockImplementationOnce(() => new Promise((resolve) => { finish = resolve }))
  const refresh = vi.fn()
  dispose = watchRemoteGit(uri, uri, refresh)
  await settle()
  dispose()
  finish({ streamId: "abandoned" })
  await settle()
  expect(invoke).toHaveBeenLastCalledWith("host_stream_close", { owner, streamId: "abandoned" })
  expect(refresh).not.toHaveBeenCalled()
})

it("reopens once after disconnect, refreshes the snapshot, and cancels pending retries", async () => {
  const refresh = vi.fn()
  dispose = watchRemoteGit(uri, uri, refresh)
  await settle()
  channel().onmessage({ type: "closed", owner })
  channel().onmessage({ type: "closed", owner })
  await vi.advanceTimersByTimeAsync(1000)
  expect(invoke).toHaveBeenCalledWith("host_stream_close", { owner, streamId: "git-watch-1" })
  expect(vi.mocked(invoke).mock.calls.filter(([name]) => name === "host_stream_open")).toHaveLength(2)
  expect(refresh).toHaveBeenCalledTimes(2)
  channel(1).onmessage({ type: "closed", owner })
  dispose()
  await vi.advanceTimersByTimeAsync(30000)
  expect(vi.mocked(invoke).mock.calls.filter(([name]) => name === "host_stream_open")).toHaveLength(2)
})

it("does not publish a watcher opened for a replaced backend", async () => {
  let finish!: (value: unknown) => void
  vi.mocked(invoke).mockImplementationOnce(() => new Promise((resolve) => { finish = resolve }))
  const refresh = vi.fn()
  dispose = watchRemoteGit(uri, uri, refresh)
  await settle()
  assertCurrent.mockImplementation(() => { throw new Error("replaced") })
  finish({ streamId: "old-generation" })
  await settle()
  expect(refresh).not.toHaveBeenCalled()
  expect(invoke).toHaveBeenCalledWith("host_stream_close", { owner, streamId: "old-generation" })
})

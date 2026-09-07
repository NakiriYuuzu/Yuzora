import { afterEach, beforeEach, expect, it, vi } from "vitest"
vi.mock("@tauri-apps/api/core", () => ({ Channel: class { onmessage = (_value: unknown) => {} } }))
vi.mock("./ipc", () => ({ invoke: vi.fn() }))
vi.mock("./remoteFiles", () => ({ runtimeWorkspaceService: vi.fn() }))
import { invoke } from "./ipc"
import { runtimeWorkspaceService } from "./remoteFiles"
import { searchRemoteWorkspace, stopRemoteSearch } from "./remoteSearch"
import { remoteFilePath } from "./runtimeIdentity"

const owner = { hostId: "search-a", generation: 2 }
const root = remoteFilePath(owner.hostId, "/project")
const assertCurrent = vi.fn()
beforeEach(() => {
  vi.clearAllMocks()
  assertCurrent.mockReset()
  vi.mocked(invoke).mockReset().mockResolvedValue({ streamId: "search" })
  vi.mocked(runtimeWorkspaceService).mockReturnValue({ owner, root: "/project", uri: root, capabilityId: "cap", assertCurrent })
})
afterEach(async () => { await stopRemoteSearch() })
function channel(index = 0) {
  const args = vi.mocked(invoke).mock.calls.filter(([name]) => name === "host_stream_open")[index][1] as { onEvent: { onmessage: (message: unknown) => void } }
  return args.onEvent
}
function frame(path: string, frameOwner = owner) {
  return { type: "frame", frame: { version: 1, owner: frameOwner, payload: { type: "search", event: { type: "match", path, matches: [] } } } }
}

it("isolates owners and rejects paths outside the workspace", async () => {
  const receive = vi.fn()
  await searchRemoteWorkspace(root, "needle", true, receive)
  channel().onmessage(frame("/project/a", { ...owner, hostId: "search-b" }))
  channel().onmessage(frame("/project/a", { ...owner, generation: 1 }))
  channel().onmessage(frame("/project-other/a"))
  expect(receive).not.toHaveBeenCalled()
  channel().onmessage(frame("/project/中文 file.txt"))
  expect(receive).toHaveBeenCalledExactlyOnceWith({ type: "match", path: root + "/%E4%B8%AD%E6%96%87%20file.txt", matches: [] })
})

it("closes a late stream after replacement and ignores its results", async () => {
  let finish!: (value: unknown) => void
  vi.mocked(invoke).mockImplementationOnce(() => new Promise((resolve) => { finish = resolve }))
  const oldEvents = vi.fn()
  const opening = searchRemoteWorkspace(root, "old", false, oldEvents)
  await searchRemoteWorkspace(root, "new", false, vi.fn())
  finish({ streamId: "late" })
  await opening
  channel(0).onmessage(frame("/project/stale"))
  expect(oldEvents).not.toHaveBeenCalled()
  expect(invoke).toHaveBeenCalledWith("host_stream_close", { owner, streamId: "late" })
})

it("cancels a pending open and releases a stream whose backend was replaced", async () => {
  let finish!: (value: unknown) => void
  vi.mocked(invoke).mockImplementationOnce(() => new Promise((resolve) => { finish = resolve }))
  const opening = searchRemoteWorkspace(root, "needle", false, vi.fn())
  await stopRemoteSearch()
  finish({ streamId: "cancelled" })
  await opening
  expect(invoke).toHaveBeenCalledWith("host_stream_close", { owner, streamId: "cancelled" })
  assertCurrent.mockImplementationOnce(() => {}).mockImplementation(() => { throw new Error("replaced") })
  await searchRemoteWorkspace(root, "needle", false, vi.fn())
  expect(invoke).toHaveBeenLastCalledWith("host_stream_close", { owner, streamId: "search" })
})

it("closes when done arrives before the open reply", async () => {
  let finish!: (value: unknown) => void
  vi.mocked(invoke).mockImplementationOnce(() => new Promise((resolve) => { finish = resolve }))
  const receive = vi.fn()
  const opening = searchRemoteWorkspace(root, "needle", false, receive)
  const event = { type: "done", truncated: false, fileCount: 3 }
  channel().onmessage({ type: "frame", frame: { version: 1, owner, payload: { type: "search", event } } })
  finish({ streamId: "finished" })
  await opening
  expect(receive).toHaveBeenCalledExactlyOnceWith(event)
  expect(invoke).toHaveBeenLastCalledWith("host_stream_close", { owner, streamId: "finished" })
})

import { afterEach, beforeEach, expect, it, vi } from "vitest"

vi.mock("@tauri-apps/api/core", () => ({ Channel: class { onmessage = (_value: unknown) => {} } }))
vi.mock("@tauri-apps/api/event", () => ({ emit: vi.fn(async () => {}) }))
vi.mock("./ipc", () => ({ invoke: vi.fn() }))
vi.mock("./remoteFiles", () => ({ runtimeWorkspaceService: vi.fn() }))
vi.mock("./remoteTrust", () => ({ requestWorkspace: vi.fn(), consumeRemoteExecutionChallenge: vi.fn() }))

import { emit } from "@tauri-apps/api/event"
import { invoke } from "./ipc"
import { runtimeWorkspaceService } from "./remoteFiles"
import { consumeRemoteExecutionChallenge, requestWorkspace } from "./remoteTrust"
import { detectRemoteDevServer, startRemoteDevServer, stopRemoteDevServer } from "./remoteDevServer"
import { remoteFilePath } from "./runtimeIdentity"

const owner = { hostId: "dev-a", generation: 4 }
const workspace = remoteFilePath(owner.hostId, "/project")
const other = remoteFilePath("dev-b", "/project")
const assertCurrent = vi.fn()
const info = { workspace: "/project", command: "bun run dev", port: 5173, status: { status: "running", port: 5173 } }
const service = { owner, uri: workspace, root: "/project", capabilityId: "cap", assertCurrent }
const settle = async () => { for (let i = 0; i < 8; i++) await Promise.resolve() }
const start = (output = vi.fn()) => startRemoteDevServer(workspace, info.command, info.port, output, "proof")
function channel(index = 0) {
  return (vi.mocked(invoke).mock.calls.filter(([command]) => command === "host_stream_open")[index][1] as { onEvent: { onmessage: (value: unknown) => void } }).onEvent
}
function frame(payload: unknown, eventOwner = owner) { return { type: "frame", frame: { version: 1, owner: eventOwner, payload } } }

beforeEach(() => {
  vi.clearAllMocks()
  assertCurrent.mockReset()
  vi.mocked(runtimeWorkspaceService).mockReturnValue(service)
  vi.mocked(consumeRemoteExecutionChallenge).mockReturnValue({ raw: "raw-proof", service })
  vi.mocked(invoke).mockReset().mockResolvedValue({ streamId: "dev", value: info })
})
afterEach(async () => { await stopRemoteDevServer(workspace); await stopRemoteDevServer(other) })

it("discovers on the workspace host and checks the execution proof before opening", async () => {
  const detected = { candidates: [], runningPorts: [5173] }
  vi.mocked(requestWorkspace).mockResolvedValue(detected)
  await expect(detectRemoteDevServer(workspace, [5173])).resolves.toBe(detected)
  expect(requestWorkspace).toHaveBeenCalledExactlyOnceWith(service, { method: "devServerDetect", params: { workspace: "cap", extra_ports: [5173] } })
  await expect(start()).resolves.toMatchObject({ workspace })
  expect(consumeRemoteExecutionChallenge).toHaveBeenCalledExactlyOnceWith("proof", workspace)
  expect(invoke).toHaveBeenCalledWith("host_stream_open", expect.objectContaining({ owner, config: { kind: "devServer", workspace: "cap", path: "/project", command: info.command, port: 5173, challengeId: "raw-proof" } }))
  await expect(start()).rejects.toThrow("already running")
})

it("isolates host, generation, workspace and command while routing status/output", async () => {
  const output = vi.fn()
  await start(output)
  for (const foreign of [{ ...owner, generation: 3 }, { ...owner, hostId: "dev-b" }]) {
    channel().onmessage(frame({ type: "devServerOutput", text: "foreign" }, foreign))
    channel().onmessage({ type: "closed", owner: foreign, reason: "foreign" })
  }
  for (const invalid of [{ ...info, workspace: "/other" }, { ...info, command: "other" }]) {
    channel().onmessage(frame({ type: "devServerStatus", info: invalid }))
  }
  expect(output).not.toHaveBeenCalled()
  expect(emit).not.toHaveBeenCalled()
  channel().onmessage(frame({ type: "devServerOutput", text: "ready" }))
  channel().onmessage(frame({ type: "devServerStatus", info }))
  expect(output).toHaveBeenCalledExactlyOnceWith("ready")
  expect(emit).toHaveBeenCalledExactlyOnceWith("dev-server:status", { ...info, workspace })
})

it("closes a late stream after stop without removing a newer start", async () => {
  let resolve!: (value: unknown) => void
  vi.mocked(invoke).mockImplementationOnce(() => new Promise((done) => { resolve = done }))
  const pending = start()
  const rejected = expect(pending).rejects.toThrow("superseded")
  await settle()
  await stopRemoteDevServer(workspace)
  await start()
  resolve({ streamId: "late", value: info })
  await rejected
  channel().onmessage({ type: "closed", owner, reason: "old" })
  expect(emit).not.toHaveBeenCalled()
  await expect(start()).rejects.toThrow("already running")
  expect(invoke).toHaveBeenLastCalledWith("host_stream_close", { owner, streamId: "late" })
})

it("discards a replaced generation and permits an explicit new start without replay", async () => {
  await start()
  assertCurrent.mockImplementation(() => { throw new Error("replaced") })
  channel().onmessage({ type: "closed", owner, reason: "disconnected" })
  expect(emit).not.toHaveBeenCalled()
  expect(vi.mocked(invoke).mock.calls.filter(([name]) => name === "host_stream_open")).toHaveLength(1)
  assertCurrent.mockReset()
  await expect(start()).resolves.toMatchObject({ workspace })
})

it("reports connection failure once and never automatically restarts", async () => {
  await start()
  channel().onmessage({ type: "closed", owner, reason: "disconnected" })
  channel().onmessage({ type: "closed", owner, reason: "duplicate" })
  expect(emit).toHaveBeenCalledExactlyOnceWith("dev-server:status", expect.objectContaining({ workspace, status: { status: "failed", reason: "disconnected" } }))
  expect(vi.mocked(invoke).mock.calls.filter(([name]) => name === "host_stream_open")).toHaveLength(1)
})

it("keeps a quick command's terminal result when it closes before open completes", async () => {
  let resolve!: (value: unknown) => void
  vi.mocked(invoke).mockImplementationOnce(() => new Promise((done) => { resolve = done }))
  const pending = start()
  const exited = { ...info, status: { status: "exited", code: 0 } }
  channel().onmessage(frame({ type: "devServerStatus", info: exited }))
  channel().onmessage({ type: "closed", owner, reason: "stream-ended" })
  resolve({ streamId: "fast", value: info })
  await expect(pending).resolves.toEqual({ ...exited, workspace })
  expect(emit).toHaveBeenCalledTimes(1)
  await expect(start()).resolves.toMatchObject({ workspace })
})

it("rejects a mismatched startup reply and frees its handle", async () => {
  vi.mocked(invoke).mockResolvedValueOnce({ streamId: "wrong", value: { ...info, workspace: "/other" } })
  await expect(start()).rejects.toThrow("identity mismatch")
  expect(invoke).toHaveBeenLastCalledWith("host_stream_close", { owner, streamId: "wrong" })
  await expect(start()).resolves.toMatchObject({ workspace })
})

it("can run the same command and path on two hosts simultaneously", async () => {
  await start()
  const otherService = { ...service, owner: { hostId: "dev-b", generation: 1 }, uri: other }
  vi.mocked(consumeRemoteExecutionChallenge).mockReturnValue({ raw: "other-proof", service: otherService })
  await expect(startRemoteDevServer(other, info.command, 5173, vi.fn(), "other-proof")).resolves.toMatchObject({ workspace: other })
  await stopRemoteDevServer(workspace)
  await expect(startRemoteDevServer(other, info.command, 5173, vi.fn(), "other-proof")).rejects.toThrow("already running")
})

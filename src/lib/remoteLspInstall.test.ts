import { afterEach, beforeEach, expect, it, vi } from "vitest"
vi.mock("@tauri-apps/api/core", () => ({ Channel: class { onmessage = (_value: unknown) => {} } }))
vi.mock("./ipc", () => ({ invoke: vi.fn() }))
vi.mock("./remoteFiles", () => ({ runtimeWorkspaceService: vi.fn() }))
vi.mock("./remoteTrust", () => ({ requestWorkspace: vi.fn(async () => null) }))
vi.mock("./remoteLsp", () => ({ restartConfiguredRemoteLsp: vi.fn(async () => {}) }))
import { invoke } from "./ipc"
import { runtimeWorkspaceService } from "./remoteFiles"
import { restartConfiguredRemoteLsp } from "./remoteLsp"
import { installRemoteLsp, cancelRemoteLspInstall } from "./remoteLspInstall"
import { remoteFilePath } from "./runtimeIdentity"
const owner = { hostId: "install-a", generation: 4 }
const context = remoteFilePath(owner.hostId, "/project")
const assertCurrent = vi.fn()
const info = { language: "rust", workspace: "/project", serverId: "rust-analyzer", status: { status: "stopped" } }
const settle = async () => { for (let i = 0; i < 12; i++) await Promise.resolve() }
beforeEach(() => {
  vi.clearAllMocks()
  assertCurrent.mockReset()
  vi.mocked(invoke).mockReset().mockResolvedValue({ streamId: "install" })
  vi.mocked(runtimeWorkspaceService).mockReturnValue({ owner, uri: context, root: "/project", capabilityId: "cap", assertCurrent })
})
afterEach(() => { cancelRemoteLspInstall(context, "rust") })
function channel() {
  const args = vi.mocked(invoke).mock.calls.find(([command]) => command === "host_stream_open")![1] as { onEvent: { onmessage: (value: unknown) => void } }
  return args.onEvent
}
function frame(payload: unknown, eventOwner = owner) { return { type: "frame", frame: { version: 1, owner: eventOwner, payload } } }

it("routes installation to the host, isolates progress and restarts only after completion", async () => {
  const progress = vi.fn()
  const install = installRemoteLsp(context, null, "rust", progress)
  await settle()
  expect(invoke).toHaveBeenCalledWith("host_stream_open", expect.objectContaining({ owner, config: { kind: "lspInstall", path: "/project", language: "rust", global: true } }))
  const event = { language: "rust", phase: "download", percent: 10, message: null }
  channel().onmessage(frame({ type: "lspInstallProgress", event }, { ...owner, generation: 3 }))
  channel().onmessage(frame({ type: "lspInstallProgress", event }, { ...owner, hostId: "other" }))
  expect(progress).not.toHaveBeenCalled()
  channel().onmessage(frame({ type: "lspInstallProgress", event }))
  expect(progress).toHaveBeenCalledExactlyOnceWith(event)
  channel().onmessage(frame({ type: "lspInstalled", outcome: { status: "ok", value: info } }))
  await expect(install).resolves.toMatchObject({ workspace: "", language: "rust" })
  expect(restartConfiguredRemoteLsp).toHaveBeenCalledExactlyOnceWith(context, true, "rust")
  expect(invoke).toHaveBeenLastCalledWith("host_stream_close", { owner, streamId: "install" })
})

it("cancels an opening stream and closes its late handle without replaying", async () => {
  let opened!: (value: unknown) => void
  vi.mocked(invoke).mockImplementationOnce(() => new Promise((resolve) => { opened = resolve }))
  const install = installRemoteLsp(context, context, "rust")
  const rejected = expect(install).rejects.toThrow("cancelled")
  await settle()
  cancelRemoteLspInstall(context, "rust")
  opened({ streamId: "late" })
  await rejected
  expect(invoke).toHaveBeenLastCalledWith("host_stream_close", { owner, streamId: "late" })
  expect(restartConfiguredRemoteLsp).not.toHaveBeenCalled()
})

it("rejects duplicate installs and drops completion from a replaced connection", async () => {
  const install = installRemoteLsp(context, context, "rust")
  const rejected = expect(install).rejects.toThrow("replaced")
  await settle()
  await expect(installRemoteLsp(context, context, "rust")).rejects.toThrow("already running")
  assertCurrent.mockImplementation(() => { throw new Error("replaced") })
  channel().onmessage(frame({ type: "lspInstalled", outcome: { status: "ok", value: info } }))
  await rejected
  expect(restartConfiguredRemoteLsp).not.toHaveBeenCalled()
})

it("reports a transport loss without restarting installation", async () => {
  const install = installRemoteLsp(context, context, "rust")
  const rejected = expect(install).rejects.toThrow("disconnected")
  await settle()
  channel().onmessage({ type: "closed", owner, reason: "disconnected" })
  await rejected
  expect(vi.mocked(invoke).mock.calls.filter(([name]) => name === "host_stream_open")).toHaveLength(1)
})

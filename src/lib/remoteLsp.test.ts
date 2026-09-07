import { beforeEach, expect, it, vi } from "vitest"
const shared = vi.hoisted(() => ({ channels: [] as Array<{ onmessage: (value: unknown) => void }>, valid: true }))
vi.mock("@tauri-apps/api/core", () => ({ Channel: class { onmessage = (_value: unknown) => {}; constructor() { shared.channels.push(this) } } }))
vi.mock("@tauri-apps/api/event", () => ({ emit: vi.fn(async () => {}) }))
vi.mock("@/lsp/lspManager", () => ({ stopWorkspace: vi.fn() }))
vi.mock("./ipc", () => ({ invoke: vi.fn() }))
vi.mock("./remoteTrust", () => ({ requestWorkspace: vi.fn(async () => ({})) }))
vi.mock("./remoteFiles", () => ({ runtimeWorkspaceService: (uri: string) => ({ owner: { hostId: "a", generation: 1 }, uri, root: "/repo", capabilityId: "cap", assertCurrent: () => { if (!shared.valid) throw new Error("stale backend") } }) }))
import { invoke } from "./ipc"
import { startRemoteLsp, sendRemoteLsp, stopRemoteLsp } from "./remoteLsp"
import { remoteFilePath } from "./runtimeIdentity"
import { stopWorkspace } from "@/lsp/lspManager"
import { emit } from "@tauri-apps/api/event"

beforeEach(() => { vi.clearAllMocks(); vi.mocked(invoke).mockResolvedValue(null); shared.channels.length = 0; shared.valid = true })
const info = { workspace: "/repo", language: "rust", serverId: "rust-analyzer", command: "rust-analyzer", status: { status: "starting" } }

it("binds messages to host/generation and sends server-native file URIs", async () => {
  const workspace = remoteFilePath("a", "/repo")
  vi.mocked(invoke).mockResolvedValueOnce({ streamId: "lsp-1", value: info })
  const message = vi.fn()
  expect(await startRemoteLsp(workspace, "rust", message)).toMatchObject({ workspace })
  const payload = { type: "lsp", message: JSON.stringify({ params: { uri: "file:///repo/a.rs", diagnostics: [] } }) }
  shared.channels[0].onmessage({ type: "frame", frame: { version: 1, owner: { hostId: "a", generation: 0 }, payload } })
  expect(message).not.toHaveBeenCalled()
  shared.channels[0].onmessage({ type: "frame", frame: { version: 1, owner: { hostId: "a", generation: 1 }, payload } })
  expect(JSON.parse(message.mock.calls[0][0]).params.uri).toBe(remoteFilePath("a", "/repo/a.rs", "/repo"))
  vi.mocked(invoke).mockResolvedValueOnce(null)
  await sendRemoteLsp(workspace, "rust", JSON.stringify({ params: { textDocument: { uri: remoteFilePath("a", "/repo/a.rs", "/repo") } } }))
  expect(invoke).toHaveBeenLastCalledWith("host_stream_command", { owner: { hostId: "a", generation: 1 }, streamId: "lsp-1", operation: { command: "lspMessage", message: JSON.stringify({ params: { textDocument: { uri: "file:///repo/a.rs" } } }) } })
  shared.valid = false
  await expect(sendRemoteLsp(workspace, "rust", "{}")).rejects.toThrow("unavailable")
  await stopRemoteLsp(workspace)
})

it("releases an LSP stream whose startup completes after its workspace closes", async () => {
  const workspace = remoteFilePath("a", "/closing")
  let resolve!: (value: unknown) => void
  vi.mocked(invoke).mockImplementationOnce(() => new Promise((complete) => { resolve = complete }))
  const opening = startRemoteLsp(workspace, "rust", vi.fn())
  await vi.waitFor(() => expect(invoke).toHaveBeenCalled())
  await stopRemoteLsp(workspace)
  resolve({ streamId: "late-lsp", value: info })
  await expect(opening).rejects.toThrow("superseded")
  expect(invoke).toHaveBeenCalledWith("host_stream_close", { owner: { hostId: "a", generation: 1 }, streamId: "late-lsp" })
})

it("clears the frontend client and reports failure when a server sends an invalid file authority", async () => {
  const workspace = remoteFilePath("a", "/invalid-uri")
  vi.mocked(invoke).mockResolvedValueOnce({ streamId: "invalid-uri", value: info })
  const onMessage = vi.fn()
  await startRemoteLsp(workspace, "rust", onMessage)
  shared.channels[0].onmessage({ type: "frame", frame: { version: 1, owner: { hostId: "a", generation: 1 }, payload: { type: "lsp", message: JSON.stringify({ params: { uri: "file://other-host/repo/a.rs" } }) } } })
  await vi.waitFor(() => expect(stopWorkspace).toHaveBeenCalledWith(workspace))
  expect(onMessage).not.toHaveBeenCalled()
  expect(emit).toHaveBeenCalledWith("lsp:server-status", expect.objectContaining({ workspace, status: { status: "crashed", reason: expect.stringContaining("authority") } }))
  await expect(sendRemoteLsp(workspace, "rust", "{}")).rejects.toThrow("unavailable")
  await stopRemoteLsp(workspace)
})

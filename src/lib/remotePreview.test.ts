import { afterEach, beforeEach, expect, it, vi } from "vitest"
vi.mock("./ipc", () => ({ invoke: vi.fn() }))
vi.mock("./hostIpc", () => ({ requestHost: vi.fn() }))
vi.mock("./remoteFiles", () => ({ runtimeWorkspaceService: vi.fn() }))
import { invoke } from "./ipc"
import { requestHost } from "./hostIpc"
import { runtimeWorkspaceService } from "./remoteFiles"
import { createRemotePreview, revokeRemotePreview, stopRemotePreviews } from "./remotePreview"
import { remoteFilePath } from "./runtimeIdentity"

const owner = { hostId: "preview-a", generation: 4 }
const uri = remoteFilePath(owner.hostId, "/repo/index.html", "/repo")
const assertCurrent = vi.fn()
const rawToken = "a".repeat(64)
beforeEach(() => {
  vi.clearAllMocks()
  assertCurrent.mockReset()
  vi.mocked(runtimeWorkspaceService).mockReturnValue({ owner, uri: remoteFilePath(owner.hostId, "/repo"), root: "/repo", capabilityId: "preview-cap", assertCurrent })
  vi.mocked(requestHost).mockResolvedValue({ token: rawToken, url: `http://127.0.0.1:32100/${rawToken}/index.html` })
  vi.mocked(invoke).mockResolvedValue({ tunnelId: "tunnel-a", localPort: 42100 })
})
afterEach(async () => { await stopRemotePreviews() })

it("creates the allowlist on the source host and revokes its token and tunnel together", async () => {
  const session = await createRemotePreview(uri)
  expect(requestHost).toHaveBeenCalledWith(owner, { method: "previewCreate", params: { workspace: "preview-cap", path: "index.html" } })
  expect(session.url).toBe(`http://127.0.0.1:42100/${rawToken}/index.html`)
  expect(session.token).toMatch(/^host-preview:/)
  expect(invoke).toHaveBeenCalledWith("host_tunnel_open", { owner, resourceOwner: session.token, endpoint: { host: "127.0.0.1", port: 32100 } })
  await revokeRemotePreview(session.token)
  expect(invoke).toHaveBeenLastCalledWith("host_tunnel_close", { owner, resourceOwner: session.token, tunnelId: "tunnel-a" })
  expect(requestHost).toHaveBeenLastCalledWith(owner, { method: "previewRevoke", params: { workspace: "preview-cap", token: rawToken } })
  const calls = vi.mocked(invoke).mock.calls.length
  await revokeRemotePreview(session.token)
  expect(invoke).toHaveBeenCalledTimes(calls)
})

it("closes a late tunnel and preview from a replaced connection", async () => {
  let opened!: (value: unknown) => void
  vi.mocked(invoke).mockImplementationOnce(() => new Promise((resolve) => { opened = resolve }))
  const pending = createRemotePreview(uri)
  const failure = expect(pending).rejects.toThrow("replaced")
  await vi.waitFor(() => expect(invoke).toHaveBeenCalled())
  assertCurrent.mockImplementation(() => { throw new Error("replaced") })
  opened({ tunnelId: "late", localPort: 42100 })
  await failure
  expect(invoke).toHaveBeenLastCalledWith("host_tunnel_close", expect.objectContaining({ owner, tunnelId: "late" }))
  expect(requestHost).toHaveBeenLastCalledWith(owner, { method: "previewRevoke", params: { workspace: "preview-cap", token: rawToken } })
})

it("rejects unexpected preview authorities before opening a tunnel", async () => {
  vi.mocked(requestHost).mockResolvedValueOnce({ token: rawToken, url: `http://external.invalid:80/${rawToken}/index.html` })
  await expect(createRemotePreview(uri)).rejects.toThrow("Invalid host preview")
  expect(invoke).not.toHaveBeenCalled()
  expect(requestHost).toHaveBeenLastCalledWith(owner, { method: "previewRevoke", params: { workspace: "preview-cap", token: rawToken } })
})

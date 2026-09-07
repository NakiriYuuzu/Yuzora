import { beforeEach, expect, it, vi } from "vitest"
vi.mock("./remoteFiles", () => ({ runtimeWorkspaceService: vi.fn() }))
vi.mock("./remoteTrust", () => ({ requestWorkspace: vi.fn() }))
vi.mock("./remoteLsp", () => ({ restartConfiguredRemoteLsp: vi.fn(async () => {}) }))
import { runtimeWorkspaceService } from "./remoteFiles"
import { requestWorkspace } from "./remoteTrust"
import { restartConfiguredRemoteLsp } from "./remoteLsp"
import { remoteLspConfigGet, remoteLspConfigSet, remoteLspConfigClear, remoteLspConfigDetect } from "./remoteLspConfig"
import { remoteFilePath } from "./runtimeIdentity"
const context = remoteFilePath("settings-a", "/project")
const service = { owner: { hostId: "settings-a", generation: 3 }, uri: context, root: "/project", capabilityId: "cap", assertCurrent: () => {} }
beforeEach(() => {
  vi.clearAllMocks()
  vi.mocked(runtimeWorkspaceService).mockReturnValue(service)
  vi.mocked(requestWorkspace).mockResolvedValue({ defaults: { rust: "rust-analyzer" }, workspaces: { "/project": { python: "pyright" } } })
})
it("projects config keys into host identity and scopes global changes to that host", async () => {
  expect((await remoteLspConfigGet(context)).workspaces[context]).toEqual({ python: "pyright" })
  await remoteLspConfigSet(context, null, "python", "pylsp")
  expect(requestWorkspace).toHaveBeenLastCalledWith(service, { method: "lspConfig", params: { workspace: "cap", call: { action: "set", language: "python", serverId: "pylsp", global: true } } })
  expect(restartConfiguredRemoteLsp).toHaveBeenCalledExactlyOnceWith(context, true, "python")
})
it("rejects settings and stale keys owned by another workspace or host", async () => {
  await expect(remoteLspConfigSet(context, remoteFilePath("settings-b", "/project"), "rust", "rust-analyzer")).rejects.toThrow("another workspace")
  await expect(remoteLspConfigClear(context, remoteFilePath("settings-b", "/gone"))).rejects.toThrow("another host")
  expect(requestWorkspace).not.toHaveBeenCalled()
})
it("probes host defaults without importing another workspace override", async () => {
  vi.mocked(requestWorkspace).mockResolvedValueOnce({ workspace: "", language: "rust" })
  expect(await remoteLspConfigDetect(context, "rust", true)).toMatchObject({ workspace: "" })
  expect(requestWorkspace).toHaveBeenLastCalledWith(service, { method: "lspConfig", params: { workspace: "cap", call: { action: "detect", language: "rust", global: true } } })
})

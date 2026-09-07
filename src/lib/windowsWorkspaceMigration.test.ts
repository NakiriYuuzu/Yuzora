import { beforeEach, expect, it, vi } from "vitest"
const mocks = vi.hoisted(() => ({ request: vi.fn(), path: vi.fn(), current: vi.fn(), entry: vi.fn(), host: { target: { kind: "wsl", distro: "Ubuntu" } } }))
vi.mock("./hostIpc", () => ({ requestHost: mocks.request, wslPath: mocks.path }))
vi.mock("./remoteFiles", () => ({ runtimeWorkspaceService: () => ({ owner: { hostId: "wsl-a", generation: 2 }, root: "/mnt/c/project", capabilityId: "active", assertCurrent: mocks.current }) }))
vi.mock("@/state/hostStore", () => ({ useHostStore: { getState: () => ({ hosts: { "wsl-a": mocks.host } }) } }))
vi.mock("@/state/workspaceSession", () => ({ loadWorkspaceSessionEntry: mocks.entry }))
import { bindWindowsWorkspace, mapWindowsSession } from "./windowsWorkspaceMigration"
import { remoteFilePath } from "./runtimeIdentity"
const root = remoteFilePath("wsl-a", "/mnt/c/project")
beforeEach(() => {
  vi.resetAllMocks()
  mocks.host.target = { kind: "wsl", distro: "Ubuntu" }
  mocks.path.mockResolvedValue("/mnt/c/project")
  mocks.request.mockResolvedValue({ canonicalPath: "/mnt/c/project", capabilityId: "verification" })
  mocks.entry.mockReturnValue({ tabs: ["C:\\project\\中文 file.ts"], activePath: "C:\\project\\中文 file.ts" })
})
it("maps only layout paths inside the explicitly bound Windows root", () => {
  const result = mapWindowsSession("C:\\project", root, { tabs: ["C:\\project\\中文 file.ts", "C:\\project2\\other.ts", "C:\\project\\..\\secret", "yuzora://herdr-terminal/a"], activePath: "C:\\project\\中文 file.ts" })
  expect(result).toEqual({ tabs: [root + "/%E4%B8%AD%E6%96%87%20file.ts"], activePath: root + "/%E4%B8%AD%E6%96%87%20file.ts" })
})
it("verifies wslpath and canonical root in the exact distro before copying tabs", async () => {
  const result = await bindWindowsWorkspace("C:\\project", root)
  expect(mocks.path).toHaveBeenCalledWith("wsl-a", "Ubuntu", "C:\\project")
  expect(result?.tabs).toHaveLength(1)
  expect(mocks.request).toHaveBeenLastCalledWith({ hostId: "wsl-a", generation: 2 }, { method: "workspaceClose", params: { workspace: "verification" } })
})
it("rejects a different selected root and still releases its verification capability", async () => {
  mocks.request.mockResolvedValueOnce({ canonicalPath: "/different", capabilityId: "verification" })
  await expect(bindWindowsWorkspace("C:\\project", root)).rejects.toThrow("does not match")
  expect(mocks.entry).not.toHaveBeenCalled()
  expect(mocks.request).toHaveBeenCalledTimes(2)
})
it("does not migrate Windows tabs into an SSH identity", async () => {
  mocks.host.target.kind = "ssh"
  await expect(bindWindowsWorkspace("C:\\project", root)).rejects.toThrow("WSL2")
  expect(mocks.path).not.toHaveBeenCalled()
})

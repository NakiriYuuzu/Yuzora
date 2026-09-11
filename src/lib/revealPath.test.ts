import { afterEach, expect, it, vi } from "vitest"
import { useWorkspaceStore } from "@/state/workspaceStore"
import { useHostStore } from "@/state/hostStore"
import { remoteFilePath } from "./runtimeIdentity"
import { revealPathInSystem, systemRevealPath } from "./revealPath"

const activateWorkspace = vi.hoisted(() => vi.fn())
const openDirectory = vi.hoisted(() => vi.fn().mockResolvedValue(undefined))
vi.mock("./ipc", () => ({ openWorkspaceDirectory: openDirectory, openWorkspace: activateWorkspace }))
const native = vi.hoisted(() => vi.fn().mockResolvedValue(undefined))
vi.mock("./hostIpc", () => ({ revealHostPath: native }))
const opener = vi.hoisted(() => vi.fn().mockResolvedValue(undefined))
vi.mock("@tauri-apps/plugin-opener", () => ({ revealItemInDir: opener }))
afterEach(() => { vi.restoreAllMocks(); useHostStore.setState({ hosts: {} }); opener.mockClear(); native.mockClear(); openDirectory.mockReset().mockResolvedValue(undefined); activateWorkspace.mockReset(); useWorkspaceStore.setState({ workspaceCapabilityId: null, workspacePath: null }) })

it("reveals WSL files in their connected distribution with Unicode and spaces intact", async () => {
  vi.spyOn(navigator, "userAgent", "get").mockReturnValue("Windows")
  useHostStore.setState({ hosts: { "wsl:identity": {
    connection: { owner: { hostId: "wsl:identity", generation: 1 }, hello: { protocol: 1, version: "1", os: "linux", arch: "x86_64", home: "/home/me", methods: [] } },
    target: { kind: "wsl", distro: "Ubuntu-24.04" }, connecting: false, error: null, attempt: 0, retryAt: 0
  } } })
  await revealPathInSystem(remoteFilePath("wsl:identity", "/home/me/中文 project/a.ts", "/home/me/中文 project"))
  expect(native).toHaveBeenCalledWith({ hostId: "wsl:identity", generation: 1 }, "/home/me/中文 project/a.ts", false)
  expect(opener).not.toHaveBeenCalled()
  expect(systemRevealPath(remoteFilePath("another-host", "/same/path"))).toBeNull()
})

it("keeps local paths and refuses disconnected remote identities", async () => {
  await revealPathInSystem("C:\\Project\\a.ts")
  expect(opener).toHaveBeenCalledWith("C:\\Project\\a.ts")
  await expect(revealPathInSystem(remoteFilePath("ssh-host", "/project/a.ts"))).rejects.toThrow("unavailable")
  expect(opener).toHaveBeenCalledTimes(1)
})

it("passes directory semantics to the native WSL route and keeps comma paths intact", async () => {
  vi.spyOn(navigator, "userAgent", "get").mockReturnValue("Mozilla/5.0 (Windows NT 10.0; Win64; x64)")
  useHostStore.setState({ hosts: { "wsl:identity": {
    connection: { owner: { hostId: "wsl:identity", generation: 2 }, hello: { protocol: 1, version: "1", os: "linux", arch: "x86_64", home: "/home/me", methods: [] } },
    target: { kind: "wsl", distro: "Ubuntu-24.04" }, connecting: false, error: null, attempt: 0, retryAt: 0
  } } })
  await revealPathInSystem(remoteFilePath("wsl:identity", "/home/me/中文, project", "/home/me/中文, project"), true)
  expect(native).toHaveBeenCalledWith({ hostId: "wsl:identity", generation: 2 }, "/home/me/中文, project", true)
  expect(opener).not.toHaveBeenCalled()
})

it.each([
  ["Windows", { kind: "ssh" as const, sessionId: "ssh-1" }],
  ["Macintosh", { kind: "wsl" as const, distro: "Ubuntu" }],
])("does not route unsupported host/platform combinations to Explorer: %s", async (agent, target) => {
  vi.spyOn(navigator, "userAgent", "get").mockReturnValue(agent)
  useHostStore.setState({ hosts: { remote: {
    connection: { owner: { hostId: "remote", generation: 1 }, hello: { protocol: 1, version: "1", os: "linux", arch: "x86_64", home: "/home/me", methods: [] } },
    target, connecting: false, error: null, attempt: 0, retryAt: 0,
  } } })
  await expect(revealPathInSystem(remoteFilePath("remote", "/home/me/file.ts"))).rejects.toThrow("unavailable")
  expect(native).not.toHaveBeenCalled()
  expect(opener).not.toHaveBeenCalled()
})

it("opens local directory contents through the current workspace capability without broad opener permissions", async () => {
  useWorkspaceStore.setState({ workspacePath: "/workspace", workspaceCapabilityId: "ws-current" })
  await revealPathInSystem("/workspace/中文 project", true)
  expect(openDirectory).toHaveBeenCalledWith("ws-current", "/workspace/中文 project")
  expect(opener).not.toHaveBeenCalled()
  useWorkspaceStore.setState({ workspacePath: null, workspaceCapabilityId: null })
  await expect(revealPathInSystem("/workspace", true)).rejects.toThrow("capability")
  expect(openDirectory).toHaveBeenCalledTimes(1)
})

it("renews a startup-restored capability rejected by the current native registry without clearing editor state", async () => {
  useWorkspaceStore.setState({ workspacePath: "/workspace", workspaceCapabilityId: "ws-old-process" })
  const groups = useWorkspaceStore.getState().groups
  openDirectory.mockRejectedValueOnce("workspace-capability-missing")
  activateWorkspace.mockResolvedValue({ canonicalPath: "/workspace", capabilityId: "ws-fresh" })
  await revealPathInSystem("/workspace", true)
  expect(activateWorkspace).toHaveBeenCalledWith("/workspace")
  expect(openDirectory).toHaveBeenNthCalledWith(1, "ws-old-process", "/workspace")
  expect(openDirectory).toHaveBeenNthCalledWith(2, "ws-fresh", "/workspace")
  expect(useWorkspaceStore.getState().workspaceCapabilityId).toBe("ws-fresh")
  expect(useWorkspaceStore.getState().groups).toBe(groups)
})

it("does not publish or open a refreshed grant after the user switches workspaces", async () => {
  useWorkspaceStore.setState({ workspacePath: "/workspace", workspaceCapabilityId: "ws-stale" })
  openDirectory.mockRejectedValueOnce("workspace-capability-missing")
  let finish!: (value: { canonicalPath: string; capabilityId: string }) => void
  activateWorkspace.mockImplementationOnce(() => new Promise(resolve => { finish = resolve }))
  const opening = revealPathInSystem("/workspace", true)
  const result = opening.then(() => null, error => error)
  await vi.waitFor(() => expect(finish).toBeDefined())
  useWorkspaceStore.setState({ workspacePath: "/another", workspaceCapabilityId: "ws-other" })
  finish({ canonicalPath: "/workspace", capabilityId: "ws-refreshed" })
  expect(await result).toBeInstanceOf(Error)
  expect((await result).message).toContain("changed")
  expect(useWorkspaceStore.getState().workspaceCapabilityId).toBe("ws-other")
  expect(openDirectory).toHaveBeenCalledTimes(1)
})

it("does not retry ordinary directory errors or renew outside the active workspace", async () => {
  useWorkspaceStore.setState({ workspacePath: "/workspace", workspaceCapabilityId: "ws-current" })
  openDirectory.mockRejectedValueOnce("permission denied")
  await expect(revealPathInSystem("/workspace", true)).rejects.toBe("permission denied")
  await expect(revealPathInSystem("/outside", true)).rejects.toThrow("workspace")
  expect(activateWorkspace).not.toHaveBeenCalled()
})

it("renews a missing startup grant and coalesces concurrent expired-grant requests", async () => {
  useWorkspaceStore.setState({ workspacePath: "/workspace", workspaceCapabilityId: null })
  activateWorkspace.mockResolvedValueOnce({ canonicalPath: "/workspace", capabilityId: "ws-startup" })
  await revealPathInSystem("/workspace", true)
  expect(openDirectory).toHaveBeenLastCalledWith("ws-startup", "/workspace")
  activateWorkspace.mockClear()
  useWorkspaceStore.setState({ workspaceCapabilityId: "ws-expired" })
  openDirectory.mockRejectedValueOnce("workspace-capability-missing").mockRejectedValueOnce("workspace-capability-missing")
  activateWorkspace.mockResolvedValueOnce({ canonicalPath: "/workspace", capabilityId: "ws-shared" })
  await Promise.all([revealPathInSystem("/workspace", true), revealPathInSystem("/workspace", true)])
  expect(activateWorkspace).toHaveBeenCalledTimes(1)
  expect(useWorkspaceStore.getState().workspaceCapabilityId).toBe("ws-shared")
})

it("bounds capability recovery to one renewal if native rejects the new grant too", async () => {
  useWorkspaceStore.setState({ workspacePath: "/workspace", workspaceCapabilityId: "ws-stale" })
  openDirectory.mockRejectedValue("workspace-capability-missing")
  activateWorkspace.mockResolvedValue({ canonicalPath: "/workspace", capabilityId: "ws-fresh" })
  await expect(revealPathInSystem("/workspace", true)).rejects.toBe("workspace-capability-missing")
  expect(activateWorkspace).toHaveBeenCalledTimes(1)
  expect(openDirectory).toHaveBeenCalledTimes(2)
})

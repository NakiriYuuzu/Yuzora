import { afterEach, beforeEach, expect, it, vi } from "vitest"
vi.hoisted(() => { vi.resetModules() })
vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn(), Channel: class {} }))
import { invoke } from "@tauri-apps/api/core"
import { closeNativeGitWorkspace, invokeNativeGit } from "./nativeGit"

beforeEach(() => { vi.mocked(invoke).mockReset() })
afterEach(() => { closeNativeGitWorkspace("/a"); closeNativeGitWorkspace("/b") })

it("keeps separate repository lifetimes and closes only the owning generation", async () => {
  vi.mocked(invoke).mockResolvedValueOnce({ workspaceGeneration: 1 }).mockResolvedValueOnce({ workspaceGeneration: 2 }).mockResolvedValue(null)
  await invokeNativeGit("git_bootstrap", { path: "/a" })
  await invokeNativeGit("git_bootstrap", { path: "/b" })
  closeNativeGitWorkspace("/a")
  expect(invoke).toHaveBeenLastCalledWith("git_close_workspace", { path: "/a", generation: 1 })
  await invokeNativeGit("git_status_cmd", { repositoryRoot: "/b", pathspec: null })
  expect(invoke).toHaveBeenLastCalledWith("git_status_cmd", { repositoryRoot: "/b", pathspec: null })
})

it("closes a late result after its workspace was closed", async () => {
  let finish!: (value: unknown) => void
  vi.mocked(invoke).mockImplementationOnce(() => new Promise((resolve) => { finish = resolve })).mockResolvedValue(null)
  const pending = invokeNativeGit("git_bootstrap", { path: "/a" })
  closeNativeGitWorkspace("/a")
  finish({ workspaceGeneration: 7 })
  await expect(pending).rejects.toThrow("changed")
  expect(invoke).toHaveBeenLastCalledWith("git_close_workspace", { path: "/a", generation: 7 })
})

it("an old detection cannot clobber or close a newer generation of the same path", async () => {
  let finish!: (value: unknown) => void
  vi.mocked(invoke).mockImplementationOnce(() => new Promise((resolve) => { finish = resolve }))
    .mockResolvedValueOnce({ workspaceGeneration: 2 }).mockResolvedValue(null)
  const old = invokeNativeGit("git_detect", { path: "/a" })
  await invokeNativeGit("git_detect", { path: "/a" })
  finish({ workspaceGeneration: 1 })
  await expect(old).rejects.toThrow("changed")
  expect(invoke).toHaveBeenLastCalledWith("git_close_workspace", { path: "/a", generation: 1 })
  closeNativeGitWorkspace("/a")
  expect(invoke).toHaveBeenLastCalledWith("git_close_workspace", { path: "/a", generation: 2 })
})

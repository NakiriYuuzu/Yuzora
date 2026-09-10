import { beforeEach, describe, expect, it, vi } from "vitest"
vi.mock("./ipc", () => ({ invoke: vi.fn(), sftpListDir: vi.fn() }))
vi.mock("@tauri-apps/api/event", () => ({ emit: vi.fn(async () => {}) }))
vi.mock("@/lsp/lspManager", () => ({ restartWorkspace: vi.fn(async () => {}) }))
vi.mock("@/state/sshStore", () => ({ useSshStore: { getState: vi.fn() } }))
import { invoke, sftpListDir } from "./ipc"
import { useSshStore } from "@/state/sshStore"
import { createRemotePath, readRemoteFile, readRemoteFileSnapshot, reconnectRemoteWorkspaces, registerRuntimeWorkspace, registerSftpWorkspace, renameRemotePath, saveRemoteFile } from "./remoteFiles"
import { remoteFilePath, parseRemoteFilePath } from "./runtimeIdentity"
import { canonicalPathKey, nativePathJoin, nativePathParent, relativePathWithin } from "./paths"

beforeEach(() => {
  vi.clearAllMocks()
  vi.mocked(useSshStore.getState).mockReturnValue({ sessions: { a: { sessionId: "a-1" }, b: { sessionId: "b-1" } } } as unknown as ReturnType<typeof useSshStore.getState>)
  vi.mocked(sftpListDir).mockResolvedValue({ cwd: "/project", entries: [] })
  vi.mocked(invoke).mockResolvedValue({ file: { kind: "full", content: "old", lineEnding: "lf", size: 3 }, revision: "original" })
})

describe("remote documents", () => {
  it("a discarded reload cannot authorize saving over an unseen remote revision", async () => {
    const owner = { hostId: "discarded-reload", generation: 1 }
    vi.mocked(invoke).mockResolvedValueOnce({ canonicalPath: "/project", capabilityId: "workspace" })
    const root = await registerRuntimeWorkspace(owner, "/project", () => true)
    const file = root + "/file.txt"
    await readRemoteFile(file)
    vi.mocked(invoke).mockResolvedValueOnce({ file: { kind: "full", content: "external", size: 8, lineEnding: "lf" }, revision: "external-revision" })
    const discarded = await readRemoteFileSnapshot(file)
    expect(discarded.result).toMatchObject({ content: "external" })
    await saveRemoteFile(file, "my edits")
    expect(invoke).toHaveBeenLastCalledWith("host_request", { owner, operation: { method: "filesWrite", params: { workspace: "workspace", path: "file.txt", content: "my edits", revision: "original" } } })
  })

  it("releases an old generation's late registration without replacing the new workspace", async () => {
    const owner = { hostId: "late-registration", generation: 1 }
    let current = true
    let finish!: (value: unknown) => void
    vi.mocked(invoke).mockImplementationOnce(() => new Promise((resolve) => { finish = resolve }))
    const old = registerRuntimeWorkspace(owner, "/project", () => current)
    current = false
    vi.mocked(invoke).mockResolvedValueOnce({ canonicalPath: "/project", capabilityId: "new" })
    const root = await registerRuntimeWorkspace({ ...owner, generation: 2 }, "/project", () => true)
    finish({ canonicalPath: "/project", capabilityId: "old" })
    await expect(old).rejects.toThrow("discarded")
    expect(invoke).toHaveBeenLastCalledWith("host_request", { owner, operation: { method: "workspaceClose", params: { workspace: "old" } } })
    await readRemoteFile(root + "/file")
    expect(invoke).toHaveBeenLastCalledWith("host_request", { owner: { ...owner, generation: 2 }, operation: { method: "filesRead", params: { workspace: "new", path: "file" } } })
  })

  it("preserves dirty buffer revisions when a canonical alias opens concurrently", async () => {
    const owner = { hostId: "alias-registration", generation: 1 }
    vi.mocked(invoke).mockResolvedValueOnce({ canonicalPath: "/project", capabilityId: "kept" })
    const root = await registerRuntimeWorkspace(owner, "/project", () => true)
    await readRemoteFile(root + "/file")
    vi.mocked(invoke).mockResolvedValueOnce({ canonicalPath: "/project", capabilityId: "redundant" })
    expect(await registerRuntimeWorkspace(owner, "/alias", () => true)).toBe(root)
    expect(invoke).toHaveBeenLastCalledWith("host_request", { owner, operation: { method: "workspaceClose", params: { workspace: "redundant" } } })
    await saveRemoteFile(root + "/file", "dirty")
    expect(invoke).toHaveBeenLastCalledWith("host_request", { owner, operation: { method: "filesWrite", params: { workspace: "kept", path: "file", content: "dirty", revision: "original" } } })
  })

  it("discards an SFTP folder listing completed after disconnection", async () => {
    let finish!: (value: Awaited<ReturnType<typeof sftpListDir>>) => void
    vi.mocked(sftpListDir).mockImplementationOnce(() => new Promise((resolve) => { finish = resolve }))
    const opening = registerSftpWorkspace("a", "/new-folder")
    vi.mocked(useSshStore.getState).mockReturnValue({ sessions: {} } as ReturnType<typeof useSshStore.getState>)
    finish({ cwd: "/new-folder", entries: [] })
    await expect(opening).rejects.toThrow("discarded")
    await expect(readRemoteFile(remoteFilePath("a", "/new-folder/file", "/new-folder"))).rejects.toThrow("Reconnect")
  })

  it("keeps revisions and capabilities separate for overlapping workspaces on one host", async () => {
    const owner = { hostId: "overlap-files", generation: 1 }
    vi.mocked(invoke).mockResolvedValueOnce({ canonicalPath: "/project", capabilityId: "outer" })
    const outer = await registerRuntimeWorkspace(owner, "/project", () => true)
    const outerFile = nativePathJoin(outer, "sub/shared.ts")
    vi.mocked(invoke).mockResolvedValueOnce({ file: { kind: "full", content: "outer", size: 5, lineEnding: "lf" }, revision: "outer-revision" })
    await readRemoteFile(outerFile)
    vi.mocked(invoke).mockResolvedValueOnce({ canonicalPath: "/project/sub", capabilityId: "inner" })
    const inner = await registerRuntimeWorkspace(owner, "/project/sub", () => true)
    const innerFile = nativePathJoin(inner, "shared.ts")
    vi.mocked(invoke).mockResolvedValueOnce({ file: { kind: "full", content: "inner", size: 5, lineEnding: "lf" }, revision: "inner-revision" })
    await readRemoteFile(innerFile)
    vi.mocked(invoke).mockResolvedValue({ revision: "saved" })
    await saveRemoteFile(outerFile, "outer dirty buffer")
    expect(invoke).toHaveBeenLastCalledWith("host_request", { owner, operation: { method: "filesWrite", params: { workspace: "outer", path: "sub/shared.ts", content: "outer dirty buffer", revision: "outer-revision" } } })
    await saveRemoteFile(innerFile, "inner dirty buffer")
    expect(invoke).toHaveBeenLastCalledWith("host_request", { owner, operation: { method: "filesWrite", params: { workspace: "inner", path: "shared.ts", content: "inner dirty buffer", revision: "inner-revision" } } })
    await expect(renameRemotePath(outer, outerFile, innerFile)).rejects.toThrow("inside its workspace")
    await expect(readRemoteFile("yuzora-fs://overlap-files/project/sub/shared.ts")).rejects.toThrow("bind")
  })
  it("re-enables saves only for unchanged revisions after reconnecting", async () => {
    const owner = { hostId: "reconnect-revisions", generation: 1 }
    vi.mocked(invoke).mockResolvedValueOnce({ canonicalPath: "/project", capabilityId: "before" })
    const root = await registerRuntimeWorkspace(owner, "/project", () => true)
    const unchanged = root + "/unchanged.txt"
    const changed = root + "/changed.txt"
    await readRemoteFile(unchanged)
    await readRemoteFile(changed)
    vi.mocked(invoke).mockResolvedValueOnce({ canonicalPath: "/project", capabilityId: "after" })
      .mockResolvedValueOnce({ revision: "original" }).mockResolvedValueOnce({ revision: "external" })
    await reconnectRemoteWorkspaces({ ...owner, generation: 2 }, () => true)
    vi.mocked(invoke).mockResolvedValueOnce({ revision: "saved" })
    await saveRemoteFile(unchanged, "mine")
    expect(invoke).toHaveBeenLastCalledWith("host_request", { owner: { ...owner, generation: 2 }, operation: { method: "filesWrite", params: { workspace: "after", path: "unchanged.txt", content: "mine", revision: "original" } } })
    await expect(saveRemoteFile(changed, "mine")).rejects.toThrow("Compare")
  })

  it("releases a reopened capability if its connection became stale during opening", async () => {
    const owner = { hostId: "reconnect-cancelled", generation: 1 }
    vi.mocked(invoke).mockResolvedValueOnce({ canonicalPath: "/project", capabilityId: "before" })
    await registerRuntimeWorkspace(owner, "/project", () => true)
    vi.mocked(invoke).mockResolvedValueOnce({ canonicalPath: "/project", capabilityId: "abandoned" })
    await reconnectRemoteWorkspaces({ ...owner, generation: 2 }, () => false)
    expect(invoke).toHaveBeenLastCalledWith("host_request", { owner: { ...owner, generation: 2 }, operation: { method: "workspaceClose", params: { workspace: "abandoned" } } })
  })
  it("refuses cross-host mutations and preserves the owning capability", async () => {
    vi.mocked(invoke).mockResolvedValue({ canonicalPath: "/project", capabilityId: "cap-a" })
    const root = await registerRuntimeWorkspace({ hostId: "runtime-a", generation: 1 }, "/project", () => true)
    await createRemotePath(root, remoteFilePath("runtime-a", "/project/new.txt", "/project"), false)
    expect(invoke).toHaveBeenLastCalledWith("host_request", { owner: { hostId: "runtime-a", generation: 1 }, operation: { method: "filesCreate", params: { workspace: "cap-a", path: "new.txt", directory: false } } })
    await expect(renameRemotePath(root, remoteFilePath("runtime-a", "/project/new.txt", "/project"), remoteFilePath("runtime-b", "/project/new.txt", "/project"))).rejects.toThrow()
    await expect(createRemotePath(root, root, true)).rejects.toThrow("inside")
  })

  it("discards a read that completes after workspace reconnection", async () => {
    await registerSftpWorkspace("a", "/project")
    let finish!: (value: unknown) => void
    vi.mocked(invoke).mockImplementationOnce(() => new Promise((resolve) => { finish = resolve }))
    const reading = readRemoteFile(remoteFilePath("a", "/project/file", "/project"))
    vi.mocked(useSshStore.getState).mockReturnValue({ sessions: { a: { sessionId: "a-2" } } } as unknown as ReturnType<typeof useSshStore.getState>)
    await registerSftpWorkspace("a", "/project")
    finish({ file: { kind: "full", content: "stale", size: 5, lineEnding: "lf" }, revision: "stale" })
    await expect(reading).rejects.toThrow("discarded")
  })
  it("keeps identity and path operations separate across hosts", () => {
    const a = remoteFilePath("a", "/project/中文 #%.txt", "/project")
    expect(parseRemoteFilePath(a)).toEqual({ hostId: "a", path: "/project/中文 #%.txt", workspaceRoot: "/project" })
    expect(canonicalPathKey(a)).toBe(a)
    expect(nativePathJoin(remoteFilePath("a", "/project"), "中文 #%.txt")).toBe(a)
    expect(nativePathParent(a)).toBe(remoteFilePath("a", "/project"))
    expect(relativePathWithin(remoteFilePath("b", "/project"), a)).toBeNull()
  })

  it("saves to the original host with the original buffer revision", async () => {
    await registerSftpWorkspace("a", "/project")
    await registerSftpWorkspace("b", "/project")
    const path = remoteFilePath("a", "/project/main.ts", "/project")
    await readRemoteFile(path)
    vi.mocked(invoke).mockResolvedValueOnce({ file: { kind: "full", content: "changed", lineEnding: "lf", size: 7 }, revision: "external" })
    await readRemoteFile(path, false)
    vi.mocked(invoke).mockResolvedValueOnce("saved")
    await saveRemoteFile(path, "mine")
    expect(invoke).toHaveBeenLastCalledWith("sftp_save_file", { sessionId: "a-1", path: "/project/main.ts", content: "mine", expectedRevision: "original" })
  })

  it("preserves the buffer when the connection has been replaced", async () => {
    await registerSftpWorkspace("a", "/project")
    const path = remoteFilePath("a", "/project/main.ts", "/project")
    await readRemoteFile(path)
    vi.mocked(useSshStore.getState).mockReturnValue({ sessions: { a: { sessionId: "a-2" } } } as unknown as ReturnType<typeof useSshStore.getState>)
    await expect(saveRemoteFile(path, "mine")).rejects.toThrow("connection changed")
    vi.mocked(invoke).mockResolvedValue({ revision: "external" })
    await registerSftpWorkspace("a", "/project")
    await expect(saveRemoteFile(path, "mine")).rejects.toThrow("Compare")
    expect(vi.mocked(invoke).mock.calls.every(([method]) => method === "sftp_open_file")).toBe(true)
  })

  it("allows an explicit save after a new SFTP connection verifies unchanged bytes", async () => {
    vi.mocked(sftpListDir).mockResolvedValue({ cwd: "/unchanged", entries: [] })
    const root = await registerSftpWorkspace("a", "/unchanged")
    await readRemoteFile(root + "/main.ts")
    vi.mocked(useSshStore.getState).mockReturnValue({ sessions: { a: { sessionId: "a-3" } } } as unknown as ReturnType<typeof useSshStore.getState>)
    await registerSftpWorkspace("a", "/unchanged")
    expect(vi.mocked(invoke).mock.calls.every(([method]) => method === "sftp_open_file")).toBe(true)
    vi.mocked(invoke).mockResolvedValueOnce("saved")
    await saveRemoteFile(root + "/main.ts", "mine")
    expect(invoke).toHaveBeenLastCalledWith("sftp_save_file", { sessionId: "a-3", path: "/unchanged/main.ts", content: "mine", expectedRevision: "original" })
  })
})

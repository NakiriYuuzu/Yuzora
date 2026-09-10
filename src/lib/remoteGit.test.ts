import { beforeEach, describe, expect, it, vi } from "vitest"
vi.mock("./ipc", () => ({ invoke: vi.fn(), sftpListDir: vi.fn() }))
vi.mock("@/state/sshStore", () => ({ useSshStore: { getState: vi.fn() } }))
import { invoke } from "./ipc"
import { registerRuntimeWorkspace } from "./remoteFiles"
import { invokeRemoteGit } from "./remoteGit"
import { remoteTrustGrant, remoteTrustRevoke, remoteTrustStatus } from "./remoteTrust"
import { remoteFilePath } from "./runtimeIdentity"

let sequence = 0
beforeEach(() => { vi.clearAllMocks() })
async function workspace() {
  const owner = { hostId: `git-host-${++sequence}`, generation: 1 }
  vi.mocked(invoke).mockResolvedValueOnce({ canonicalPath: "/project", capabilityId: "cap-1" })
  const uri = await registerRuntimeWorkspace(owner, "/project", () => true)
  return { owner, uri }
}

describe("remote Git and trust ownership", () => {
  it("routes roots and mutations to the owning workspace and discards replaced responses", async () => {
    const { owner, uri } = await workspace()
    vi.mocked(invoke).mockResolvedValueOnce({ status: "ready", root: "/project", version: "git version 2.50" })
    expect(await invokeRemoteGit("git_detect", { path: uri })).toMatchObject({ root: uri })
    vi.mocked(invoke).mockResolvedValueOnce(null)
    await invokeRemoteGit("git_stage", { repositoryRoot: uri, paths: ["中文 file.txt"] })
    expect(invoke).toHaveBeenLastCalledWith("host_request", { owner, operation: { method: "git", params: { workspace: "cap-1", repository_root: "/project", call: { command: "git_stage", args: { paths: ["中文 file.txt"] } } } } })
    let complete!: (value: unknown) => void
    vi.mocked(invoke).mockImplementationOnce(() => new Promise((resolve) => { complete = resolve }))
    const pending = invokeRemoteGit("git_status_cmd", { repositoryRoot: uri, pathspec: null })
    vi.mocked(invoke).mockResolvedValueOnce({ canonicalPath: "/project", capabilityId: "cap-2" })
    await registerRuntimeWorkspace({ ...owner, generation: 2 }, "/project", () => true)
    complete({ branch: "old-host" })
    await expect(pending).rejects.toThrow("discarded")
    await expect(invokeRemoteGit("git_stage", { repositoryRoot: uri, paths: ["a"] })).rejects.toThrow("Refresh Git")
  })

  it("rediscovers reads after reconnect and refuses a changed repository", async () => {
    const { owner, uri } = await workspace()
    vi.mocked(invoke).mockResolvedValueOnce({ status: "ready", root: "/project", version: "v" })
    await invokeRemoteGit("git_detect", { path: uri })
    vi.mocked(invoke).mockResolvedValueOnce({ canonicalPath: "/project", capabilityId: "cap-2" })
    await registerRuntimeWorkspace({ ...owner, generation: 2 }, "/project", () => true)
    vi.mocked(invoke).mockResolvedValueOnce({ status: "ready", root: "/different", version: "v" })
    await expect(invokeRemoteGit("git_status_cmd", { repositoryRoot: uri, pathspec: null })).rejects.toThrow("changed after reconnecting")
  })

  it("projects trust errors and grants on the host that issued the challenge", async () => {
    const { owner, uri } = await workspace()
    vi.mocked(invoke).mockRejectedValueOnce(JSON.stringify({ error: "untrustedWorkspace", canonicalPath: "/project", challengeId: "raw-challenge" }))
    const result = await invokeRemoteGit("git_detect", { path: uri }).catch(String)
    const error = JSON.parse(result as string)
    expect(error.canonicalPath).toBe(uri)
    expect(JSON.parse(error.challengeId)).toEqual([owner.hostId, 1, "raw-challenge"])
    vi.mocked(invoke).mockResolvedValueOnce({ state: "trusted", canonicalPath: "/project" })
    expect(await remoteTrustGrant(error.challengeId)).toMatchObject({ canonicalPath: uri })
    expect(invoke).toHaveBeenLastCalledWith("host_request", { owner, operation: { method: "trust", params: { call: { action: "grant", challenge: "raw-challenge" } } } })
    await expect(remoteTrustGrant(error.challengeId)).rejects.toThrow("expired")
  })

  it("rejects old challenges and revokes the exact trusted path even without an open folder", async () => {
    const { owner, uri } = await workspace()
    vi.mocked(invoke).mockResolvedValueOnce({ state: "untrusted", canonicalPath: "/project", challengeId: "old" })
    const challenge = await remoteTrustStatus(uri)
    vi.mocked(invoke).mockResolvedValueOnce({ canonicalPath: "/project", capabilityId: "cap-2" })
    await registerRuntimeWorkspace({ ...owner, generation: 2 }, "/project", () => true)
    await expect(remoteTrustGrant(challenge.challengeId!)).rejects.toThrow("discarded")
    vi.mocked(invoke).mockResolvedValueOnce(null)
    await remoteTrustRevoke(remoteFilePath(owner.hostId, "/closed-folder"))
    expect(invoke).toHaveBeenLastCalledWith("host_request", { owner: { ...owner, generation: 2 }, operation: { method: "trust", params: { call: { action: "revoke", path: "/closed-folder" } } } })
  })
})

import { beforeEach, describe, expect, it, vi } from "vitest"
import { remoteFilePath } from "./runtimeIdentity"

const { invoke, assertCurrent, release, retainRemoteWorkspace, source, state } = vi.hoisted(() => ({
  invoke: vi.fn(), assertCurrent: vi.fn(), release: vi.fn(), retainRemoteWorkspace: vi.fn(),
  source: { kind: "runtime" },
  state: { hosts: { h: { connection: { owner: { hostId: "h", generation: 1 }, hello: { methods: ["fileNameSearch"] } } } } },
}))
vi.mock("./ipc", () => ({ invoke }))
vi.mock("@/state/hostStore", () => ({ useHostStore: { getState: () => state } }))
vi.mock("./remoteFiles", () => ({
  remotePreviewSource: () => ({ source }),
  runtimeWorkspaceService: () => ({ owner: { hostId: "h", generation: 1 }, capabilityId: "cap", root: "/w", assertCurrent }),
  retainRemoteWorkspace,
}))
import { searchRemoteFileNames } from "./remoteFileNameSearch"
const root = remoteFilePath("h", "/w")

beforeEach(() => {
  vi.clearAllMocks()
  assertCurrent.mockReset()
  source.kind = "runtime"
  state.hosts.h.connection.owner.generation = 1
  state.hosts.h.connection.hello.methods = ["fileNameSearch"]
  retainRemoteWorkspace.mockReturnValue(release)
  invoke.mockResolvedValue({ files: [{ name: "Target.ts", path: "/w/src/Target.ts", isDir: false, kind: "file" }], incomplete: false })
})

describe("searchRemoteFileNames", () => {
  it("uses the workspace capability and maps absolute helper paths to remote URIs", async () => {
    const result = await searchRemoteFileNames(root, "target")
    expect(invoke).toHaveBeenCalledExactlyOnceWith("host_request", {
      owner: { hostId: "h", generation: 1 }, operation: { method: "fileNameSearch", params: { workspace: "cap", query: "target" } },
    })
    expect(result?.files[0].path).toBe(remoteFilePath("h", "/w/src/Target.ts", "/w"))
    expect(assertCurrent).toHaveBeenCalledTimes(2)
    expect(release).toHaveBeenCalledTimes(1)
  })

  it("returns fallback only for older helpers and SFTP", async () => {
    state.hosts.h.connection.hello.methods = ["filesList"]
    expect(await searchRemoteFileNames(root, "target")).toBeNull()
    source.kind = "sftp"
    expect(await searchRemoteFileNames(root, "target")).toBeNull()
    expect(invoke).not.toHaveBeenCalled()
  })

  it("rejects mismatched connection generations rather than falling back", async () => {
    state.hosts.h.connection.owner.generation = 2
    await expect(searchRemoteFileNames(root, "target")).rejects.toThrow("connection changed")
    expect(invoke).not.toHaveBeenCalled()
  })

  it("discards replies after reconnect and releases the retained capability", async () => {
    assertCurrent.mockImplementationOnce(() => {}).mockImplementationOnce(() => { throw new Error("connection changed") })
    await expect(searchRemoteFileNames(root, "target")).rejects.toThrow("connection changed")
    expect(release).toHaveBeenCalledTimes(1)
  })

  it("does not expose out-of-workspace files or non-regular results", async () => {
    invoke.mockResolvedValue({ files: [
      { name: "target", path: "/outside/target", isDir: false, kind: "file" },
      { name: "target", path: "/w/target", isDir: false, kind: "symlink" },
    ], incomplete: true })
    expect(await searchRemoteFileNames(root, "target")).toEqual({ files: [], incomplete: true })
  })

  it("propagates supported helper failures without legacy fallback", async () => {
    invoke.mockRejectedValueOnce(new Error("host-error"))
    await expect(searchRemoteFileNames(root, "target")).rejects.toThrow("host-error")
    expect(release).toHaveBeenCalledTimes(1)
  })
})

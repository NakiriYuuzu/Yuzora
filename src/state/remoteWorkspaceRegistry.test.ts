import { beforeEach, describe, expect, it } from "vitest"
import { remoteFilePath } from "@/lib/runtimeIdentity"
import { loadRemoteWorkspaces, rememberRemoteWorkspace, REMOTE_WORKSPACES_STORAGE_KEY } from "./remoteWorkspaceRegistry"

beforeEach(() => {
  const values = new Map<string, string>()
  Object.defineProperty(globalThis, "localStorage", { configurable: true, value: {
    getItem: (key: string) => values.get(key) ?? null,
    setItem: (key: string, value: string) => values.set(key, value)
  } })
})

describe("remote workspace registry", () => {
  it("persists scoped identity and access without connection authority", () => {
    const a = remoteFilePath("a", "/project")
    const b = remoteFilePath("b", "/project")
    rememberRemoteWorkspace(a, "runtime")
    rememberRemoteWorkspace(b, "sftp")
    expect(loadRemoteWorkspaces()).toEqual({ [a]: "runtime", [b]: "sftp" })
    expect(() => rememberRemoteWorkspace(a + "/file", "runtime")).toThrow("scoped")
  })

  it("rejects malformed, unscoped and non-root records without losing valid rows", () => {
    const valid = remoteFilePath("valid", "/project")
    localStorage.setItem(REMOTE_WORKSPACES_STORAGE_KEY, JSON.stringify({
      "yuzora-fs://a@%ZZ/project": "runtime", "yuzora-fs://a/project": "runtime",
      [valid + "/file"]: "sftp", [remoteFilePath("unknown", "/project")]: { capabilityId: "stale" },
      [valid]: "sftp"
    }))
    expect(loadRemoteWorkspaces()).toEqual({ [valid]: "sftp" })
  })

  it("bounds recent identities and moves a revisited folder to the end", () => {
    for (let i = 0; i < 100; i++) rememberRemoteWorkspace(remoteFilePath("a", `/p${i}`), "runtime")
    rememberRemoteWorkspace(remoteFilePath("a", "/p0"), "sftp")
    rememberRemoteWorkspace(remoteFilePath("a", "/p100"), "runtime")
    const entries = loadRemoteWorkspaces()
    expect(Object.keys(entries)).toHaveLength(100)
    expect(entries[remoteFilePath("a", "/p1")]).toBeUndefined()
    expect(entries[remoteFilePath("a", "/p0")]).toBe("sftp")
  })
})

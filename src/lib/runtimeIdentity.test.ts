import { describe, expect, it } from "vitest"
import { runtimeKey, sameConnection, remoteFilePath, parseRemoteFilePath } from "./runtimeIdentity"
import { nativePathJoin, relativePathWithin, samePathIdentity } from "./paths"

describe("runtime authority", () => {
  it("isolates a shared file in overlapping workspaces and preserves encoded authorities", () => {
    const outer = remoteFilePath("主機@a", "/repo/sub/shared.ts", "/repo")
    const inner = remoteFilePath("主機@a", "/repo/sub/shared.ts", "/repo/sub")
    expect(outer).not.toBe(inner)
    expect(samePathIdentity(outer, inner)).toBe(false)
    expect(relativePathWithin(remoteFilePath("主機@a", "/repo"), inner)).toBeNull()
    expect(parseRemoteFilePath(inner)).toEqual({ hostId: "主機@a", path: "/repo/sub/shared.ts", workspaceRoot: "/repo/sub" })
    expect(nativePathJoin(remoteFilePath("主機@a", "/repo/sub"), "shared.ts")).toBe(inner)
    expect(parseRemoteFilePath("yuzora-fs://a/repo/sub/shared.ts")?.workspaceRoot).toBeNull()
  })
  it("isolates duplicate session and path names across hosts", () => {
    expect(runtimeKey({ hostId: "one", sessionName: "default" }))
      .not.toBe(runtimeKey({ hostId: "two", sessionName: "default" }))
  })

  it("does not collide on delimiters or accept an old connection generation", () => {
    expect(runtimeKey({ hostId: "a:b", sessionName: "c" }))
      .not.toBe(runtimeKey({ hostId: "a", sessionName: "b:c" }))
    expect(sameConnection({ hostId: "a", generation: 1 }, { hostId: "a", generation: 2 })).toBe(false)
    expect(sameConnection({ hostId: "a", generation: 1 }, { hostId: "b", generation: 1 })).toBe(false)
  })
})

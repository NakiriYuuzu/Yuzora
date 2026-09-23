import { describe, expect, it } from "vitest"
import { runtimeKey, sameConnection, remoteFilePath, parseRemoteFilePath, relativeRemoteHostPath } from "./runtimeIdentity"
import { nativePathJoin, nativePathParent, relativePathWithin, samePathIdentity } from "./paths"

describe("runtime authority", () => {
  it("round-trips Windows host paths and keeps child documents within their workspace", () => {
    for (const root of [String.raw`C:\Work\中文 project`, String.raw`\\?\C:\Work\中文 project`, String.raw`\\server\share\Work`]) {
      const file = `${root}\\src\\file.ts`
      const uri = remoteFilePath("windows-host", file, root)
      expect(parseRemoteFilePath(uri)).toEqual({ hostId: "windows-host", path: file, workspaceRoot: root })
      expect(relativeRemoteHostPath(root, file)).toBe("src/file.ts")
      expect(relativePathWithin(remoteFilePath("windows-host", root), uri)).toBe("src/file.ts")
      expect(nativePathJoin(nativePathJoin(remoteFilePath("windows-host", root), "src"), "file.ts")).toBe(uri)
      expect(relativeRemoteHostPath(root, `${root}-other\\file.ts`)).toBeNull()
      expect(() => remoteFilePath("windows-host", `${root}\\..\\secret`, root)).toThrow()
    }
  })
  it("compares Windows host paths case-insensitively while POSIX host paths stay case-sensitive", () => {
    expect(relativeRemoteHostPath(String.raw`C:\Work`, String.raw`c:\work\Src\file.ts`)).toBe("Src/file.ts")
    expect(relativeRemoteHostPath(String.raw`\\?\C:\Work`, String.raw`\\?\c:\WORK`)).toBe("")
    expect(relativeRemoteHostPath(String.raw`\\Server\Share\Work`, String.raw`\\server\share\work\file.ts`)).toBe("file.ts")
    expect(relativeRemoteHostPath("/Work", "/work/file.ts")).toBeNull()
    expect(relativeRemoteHostPath("/", "/work/file.ts")).toBe("work/file.ts")
  })
  it("walks Windows host parents and joins children at drive roots", () => {
    const root = String.raw`C:\Work`
    expect(nativePathParent(remoteFilePath("windows-host", String.raw`C:\Work\src\file.ts`, root))).toBe(remoteFilePath("windows-host", String.raw`C:\Work\src`, root))
    expect(nativePathParent(remoteFilePath("windows-host", root))).toBe(remoteFilePath("windows-host", "C:\\", root))
    const share = String.raw`\\server\share\Work`
    expect(nativePathParent(remoteFilePath("windows-host", share))).toBe(remoteFilePath("windows-host", String.raw`\\server\share`, share))
    for (const drive of ["C:\\", "\\\\?\\C:\\"]) {
      const file = remoteFilePath("windows-host", `${drive}data.db`, drive)
      expect(nativePathJoin(remoteFilePath("windows-host", drive), "data.db")).toBe(file)
      expect(nativePathParent(file)).toBe(remoteFilePath("windows-host", drive))
    }
  })
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

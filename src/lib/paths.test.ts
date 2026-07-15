import { describe, expect, it } from "vitest"

import {
  canonicalPathKey,
  firstAbsolutePath,
  isAbsolutePath,
  workspacePathBasename,
  workspacePathForDisplay,
} from "./paths"

describe("isAbsolutePath", () => {
  it("accepts posix absolute paths", () => {
    expect(isAbsolutePath("/workspace")).toBe(true)
    expect(isAbsolutePath("/")).toBe(true)
  })

  it("accepts Windows drive-letter paths with either slash", () => {
    expect(isAbsolutePath("C:\\Users\\me")).toBe(true)
    expect(isAbsolutePath("d:/projects")).toBe(true)
  })

  it("accepts Windows verbatim and UNC prefixes (canonicalize output)", () => {
    // std::fs::canonicalize returns the \\?\ extended-length form on Windows.
    expect(isAbsolutePath("\\\\?\\C:\\Users\\me\\proj")).toBe(true)
    expect(isAbsolutePath("\\\\server\\share")).toBe(true)
  })

  it("rejects relative paths and empty values", () => {
    expect(isAbsolutePath(".")).toBe(false)
    expect(isAbsolutePath("./src")).toBe(false)
    expect(isAbsolutePath("workspace")).toBe(false)
    expect(isAbsolutePath("")).toBe(false)
    expect(isAbsolutePath(null)).toBe(false)
    expect(isAbsolutePath(undefined)).toBe(false)
  })
})

describe("firstAbsolutePath", () => {
  it("returns the first absolute candidate in order", () => {
    expect(firstAbsolutePath(".", null, "/workspace", "/other")).toBe("/workspace")
    expect(firstAbsolutePath(null, "C:\\repo")).toBe("C:\\repo")
  })

  it("returns null when no candidate is absolute", () => {
    expect(firstAbsolutePath(".", "workspace", null, undefined)).toBeNull()
    expect(firstAbsolutePath()).toBeNull()
  })
})

describe("workspacePathForDisplay", () => {
  it("removes only the Windows extended drive prefix", () => {
    expect(workspacePathForDisplay("\\\\?\\C:\\Users\\Yuuzu\\專案 空間")).toBe(
      "C:\\Users\\Yuuzu\\專案 空間"
    )
    expect(workspacePathForDisplay("//?/D:/Work/Repo")).toBe("D:/Work/Repo")
  })

  it("converts extended UNC to standard UNC without changing the share path", () => {
    expect(workspacePathForDisplay("\\\\?\\UNC\\Server\\Share\\專案 空間")).toBe(
      "\\\\Server\\Share\\專案 空間"
    )
    expect(workspacePathForDisplay("//?/UNC/Server/Share/Repo")).toBe(
      "//Server/Share/Repo"
    )
  })

  it("leaves ordinary drive, UNC, and POSIX paths unchanged", () => {
    expect(workspacePathForDisplay("C:\\Work\\Repo")).toBe("C:\\Work\\Repo")
    expect(workspacePathForDisplay("\\\\Server\\Share\\Repo")).toBe(
      "\\\\Server\\Share\\Repo"
    )
    expect(workspacePathForDisplay("/Users/yuuzu/Repo")).toBe("/Users/yuuzu/Repo")
    expect(workspacePathForDisplay("//?/workspace")).toBe("//?/workspace")
  })
})

describe("workspacePathBasename", () => {
  it("extracts a display name with either path separator", () => {
    expect(workspacePathBasename("\\\\?\\C:\\Work\\專案 空間")).toBe("專案 空間")
    expect(workspacePathBasename("/Users/yuuzu/My Repo/")).toBe("My Repo")
    expect(workspacePathBasename("\\\\?\\UNC\\Server\\Share\\Repo\\")).toBe("Repo")
  })

  it("returns meaningful drive, share, and POSIX roots", () => {
    expect(workspacePathBasename("C:\\")).toBe("C:")
    expect(workspacePathBasename("\\\\Server\\Share\\")).toBe("Share")
    expect(workspacePathBasename("/")).toBe("/")
  })
})

describe("canonicalPathKey", () => {
  it("treats extended and ordinary Windows drive aliases as the same identity", () => {
    expect(canonicalPathKey("\\\\?\\C:\\Work\\Repo\\")).toBe(
      canonicalPathKey("c:/work/repo")
    )
  })

  it("treats extended and ordinary Windows drive roots as the same identity", () => {
    expect(canonicalPathKey("\\\\?\\C:\\")).toBe(canonicalPathKey("c:/"))
  })

  it("treats extended and standard UNC aliases as the same identity", () => {
    expect(canonicalPathKey("\\\\?\\UNC\\Server\\Share\\Repo")).toBe(
      canonicalPathKey("\\\\server\\share\\repo\\")
    )
  })

  it("keeps POSIX identity case-sensitive", () => {
    expect(canonicalPathKey("/Work/Repo")).not.toBe(canonicalPathKey("/work/repo"))
  })

  it("does not treat a generic POSIX double-slash path as a Windows drive prefix", () => {
    expect(canonicalPathKey("//?/WorkSpace")).toBe("//?/WorkSpace")
  })
})

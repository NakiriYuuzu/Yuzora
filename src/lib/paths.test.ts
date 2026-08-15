import { describe, expect, it } from "vitest"

import {
  canonicalPathKey,
  firstAbsolutePath,
  isAbsolutePath,
  isSafeLeafName,
  isSameOrDescendantPath,
  localPathJoin,
  localPathParent,
  nativePathJoin,
  nativePathParent,
  rebasePath,
  relativePathWithin,
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

  it("keeps generic and implementation-defined double-slash POSIX paths case-sensitive", () => {
    expect(canonicalPathKey("//?/WorkSpace")).toBe("//?/WorkSpace")
    expect(canonicalPathKey("//CaseHost/Share/File.ts")).not.toBe(
      canonicalPathKey("//casehost/share/file.ts")
    )
  })

  it("keeps exact // distinct while normalizing three-or-more slashes to POSIX root", () => {
    expect(canonicalPathKey("//")).toBe("//")
    expect(canonicalPathKey("//")).not.toBe(canonicalPathKey("/"))
    expect(canonicalPathKey("///tmp")).toBe(canonicalPathKey("/tmp"))
    expect(canonicalPathKey("////tmp//nested")).toBe(canonicalPathKey("/tmp/nested"))
  })
})

describe("nativePathJoin / localPathJoin", () => {
  it("joins POSIX directories with a forward slash", () => {
    expect(nativePathJoin("/work/repo", "src")).toBe("/work/repo/src")
    expect(nativePathJoin("/work/repo/", "src/a.ts")).toBe("/work/repo/src/a.ts")
    expect(localPathJoin("/", "tmp")).toBe("/tmp")
    expect(localPathJoin("//", "CaseHost")).toBe("//CaseHost")
  })

  it("joins Windows drive paths with a backslash", () => {
    expect(nativePathJoin("C:\\Work\\Repo", "src")).toBe("C:\\Work\\Repo\\src")
    expect(nativePathJoin("C:\\", "Users")).toBe("C:\\Users")
    expect(nativePathJoin("C:", "Users")).toBe("C:\\Users")
  })

  it("joins UNC and verbatim paths without rewriting operational form", () => {
    expect(nativePathJoin("\\\\Server\\Share\\Repo", "src")).toBe(
      "\\\\Server\\Share\\Repo\\src"
    )
    expect(nativePathJoin("\\\\?\\C:\\Work", "a.ts")).toBe("\\\\?\\C:\\Work\\a.ts")
  })
})

describe("nativePathParent / localPathParent", () => {
  it("returns POSIX parents including root", () => {
    expect(nativePathParent("/a/b/c")).toBe("/a/b")
    expect(nativePathParent("/a")).toBe("/")
    expect(nativePathParent("/")).toBe("/")
  })

  it("returns Windows drive parents including drive root", () => {
    expect(nativePathParent("C:\\Work\\Repo\\src")).toBe("C:\\Work\\Repo")
    expect(nativePathParent("C:\\Work")).toBe("C:\\")
    expect(nativePathParent("C:\\")).toBe("C:\\")
  })

  it("returns UNC share roots without climbing past the share", () => {
    expect(nativePathParent("\\\\Server\\Share\\Repo\\src")).toBe(
      "\\\\Server\\Share\\Repo"
    )
    expect(nativePathParent("\\\\Server\\Share\\Repo")).toBe("\\\\Server\\Share")
    expect(nativePathParent("\\\\Server\\Share")).toBe("\\\\Server\\Share")
  })

  it("handles verbatim drive paths", () => {
    expect(localPathParent("\\\\?\\C:\\Work\\Repo")).toBe("\\\\?\\C:\\Work")
    expect(localPathParent("\\\\?\\C:\\Work")).toBe("\\\\?\\C:\\")
  })
})

describe("isSameOrDescendantPath", () => {
  it("matches exact and descendant paths across separators", () => {
    expect(isSameOrDescendantPath("/work/repo", "/work/repo")).toBe(true)
    expect(isSameOrDescendantPath("/work/repo", "/work/repo/src/a.ts")).toBe(true)
    expect(isSameOrDescendantPath("C:\\Work\\Repo", "C:\\Work\\Repo\\src\\a.ts")).toBe(true)
    expect(isSameOrDescendantPath("\\\\?\\C:\\Work\\Repo", "C:\\Work\\Repo\\src")).toBe(true)
  })

  it("accepts descendants of POSIX, drive, and verbatim drive roots", () => {
    expect(isSameOrDescendantPath("/", "/Src/App.ts")).toBe(true)
    expect(isSameOrDescendantPath("C:\\", "C:\\Src\\App.ts")).toBe(true)
    expect(isSameOrDescendantPath("\\\\?\\C:\\", "C:\\Src\\App.ts")).toBe(true)
  })

  it("does not treat sibling prefixes as descendants", () => {
    expect(isSameOrDescendantPath("/work/repo", "/work/repo2")).toBe(false)
    expect(isSameOrDescendantPath("C:\\Work\\Repo", "C:\\Work\\Repo2\\a.ts")).toBe(false)
  })

  it("keeps POSIX identity case-sensitive", () => {
    expect(isSameOrDescendantPath("/Work/Repo", "/work/repo/src")).toBe(false)
  })

  it("keeps single-slash and implementation-defined double-slash POSIX namespaces separate", () => {
    expect(isSameOrDescendantPath("/", "//CaseHost/Share/File.ts")).toBe(false)
    expect(isSameOrDescendantPath("//", "//")).toBe(true)
    expect(isSameOrDescendantPath("//", "//CaseHost/Share/File.ts")).toBe(true)
    expect(isSameOrDescendantPath("//CaseHost", "//CaseHost/Share/File.ts")).toBe(true)
    expect(isSameOrDescendantPath("//CaseHost", "//CaseHost2/Share/File.ts")).toBe(false)
    expect(isSameOrDescendantPath("//CaseHost", "/CaseHost/Share/File.ts")).toBe(false)
  })

  it("treats three-or-more leading slashes as the ordinary POSIX namespace", () => {
    expect(isSameOrDescendantPath("/", "///tmp")).toBe(true)
    expect(isSameOrDescendantPath("/tmp", "////tmp/child")).toBe(true)
    expect(isSameOrDescendantPath("//", "///tmp")).toBe(false)
  })

  it("compares Unicode Windows segments without relying on folded string lengths", () => {
    expect(isSameOrDescendantPath("C:\\i\u0307", "c:\\İ\\Camel.ts")).toBe(true)
    expect(isSameOrDescendantPath("C:\\İ", "c:\\i\u0307\\Camel.ts")).toBe(true)
  })
})

describe("rebasePath", () => {
  it("rewrites exact and descendant POSIX paths", () => {
    expect(rebasePath("/old", "/new", "/old")).toBe("/new")
    expect(rebasePath("/old", "/new", "/old/src/a.ts")).toBe("/new/src/a.ts")
    expect(rebasePath("//", "//", "//CaseHost/Share/File.ts")).toBe(
      "//CaseHost/Share/File.ts"
    )
  })

  it("preserves Windows separators and descendant casing on the target root", () => {
    expect(rebasePath("C:\\Old", "C:\\New", "C:\\Old\\Src\\CamelCase.TS")).toBe(
      "C:\\New\\Src\\CamelCase.TS"
    )
    expect(
      rebasePath("\\\\?\\C:\\Old", "\\\\?\\C:\\New", "c:\\old\\Lib\\Nested\\CamelCase.TS")
    ).toBe("\\\\?\\C:\\New\\Lib\\Nested\\CamelCase.TS")
  })

  it("preserves complete suffixes across Unicode length-changing Windows folds", () => {
    expect(rebasePath("C:\\i\u0307", "C:\\Renamed", "c:\\İ\\Camel.ts")).toBe(
      "C:\\Renamed\\Camel.ts"
    )
    expect(rebasePath("C:\\İ", "C:\\Renamed", "c:\\i\u0307\\Camel.ts")).toBe(
      "C:\\Renamed\\Camel.ts"
    )
  })

  it("returns null outside the source root", () => {
    expect(rebasePath("/old", "/new", "/other/src")).toBeNull()
  })
})

describe("relativePathWithin", () => {
  it("returns Git-style forward-slash relatives", () => {
    expect(relativePathWithin("/work/repo", "/work/repo/src/a.ts")).toBe("src/a.ts")
    expect(relativePathWithin("C:\\Work\\Repo", "C:\\Work\\Repo\\src\\a.ts")).toBe(
      "src/a.ts"
    )
    expect(
      relativePathWithin("\\\\?\\C:\\Work\\Repo", "C:\\Work\\Repo\\src\\a.ts")
    ).toBe("src/a.ts")
  })

  it("returns relatives from POSIX, drive, and verbatim drive roots", () => {
    expect(relativePathWithin("/", "/Src/App.ts")).toBe("Src/App.ts")
    expect(relativePathWithin("C:\\", "C:\\Src\\CamelCase.TS")).toBe("Src/CamelCase.TS")
    expect(relativePathWithin("\\\\?\\C:\\", "c:\\Src\\CamelCase.TS")).toBe("Src/CamelCase.TS")
  })

  it("preserves mixed-case Windows descendant segments", () => {
    expect(
      relativePathWithin("C:\\Work\\Repo", "c:\\work\\repo\\Src\\CamelCase.TS")
    ).toBe("Src/CamelCase.TS")
  })

  it("preserves complete suffixes across both Unicode Windows fold directions", () => {
    expect(relativePathWithin("C:\\i\u0307", "c:\\İ\\Camel.ts")).toBe("Camel.ts")
    expect(relativePathWithin("C:\\İ", "c:\\i\u0307\\Camel.ts")).toBe("Camel.ts")
  })

  it("does not make POSIX double-slash paths relative to the single-slash root", () => {
    expect(relativePathWithin("/", "//CaseHost/Share/File.ts")).toBeNull()
    expect(relativePathWithin("//", "//")).toBe("")
    expect(relativePathWithin("//", "//CaseHost/Share/File.ts")).toBe(
      "CaseHost/Share/File.ts"
    )
    expect(relativePathWithin("//CaseHost", "//CaseHost/Share/File.ts")).toBe(
      "Share/File.ts"
    )
    expect(relativePathWithin("//CaseHost", "//CaseHost2/Share/File.ts")).toBeNull()
  })

  it("returns ordinary POSIX relatives for three-or-more leading slashes", () => {
    expect(relativePathWithin("/", "///tmp/a.ts")).toBe("tmp/a.ts")
    expect(relativePathWithin("/tmp", "////tmp/nested/a.ts")).toBe("nested/a.ts")
  })

  it("returns empty string for the root itself and null outside", () => {
    expect(relativePathWithin("/work/repo", "/work/repo")).toBe("")
    expect(relativePathWithin("/work/repo", "/work/other")).toBeNull()
  })

  it("supports repo-subdirectory workspaces against a higher git root", () => {
    expect(
      relativePathWithin("C:\\Work\\Repo", "C:\\Work\\Repo\\packages\\app\\src\\main.ts")
    ).toBe("packages/app/src/main.ts")
  })
})

describe("isSafeLeafName", () => {
  it("accepts ordinary file names", () => {
    expect(isSafeLeafName("report.pdf")).toBe(true)
    expect(isSafeLeafName("中文.txt")).toBe(true)
  })

  it("rejects traversal, separators, drive, UNC, and verbatim forms", () => {
    expect(isSafeLeafName("")).toBe(false)
    expect(isSafeLeafName(".")).toBe(false)
    expect(isSafeLeafName("..")).toBe(false)
    expect(isSafeLeafName("../.ssh/config")).toBe(false)
    expect(isSafeLeafName("foo/bar")).toBe(false)
    expect(isSafeLeafName("foo\\bar")).toBe(false)
    expect(isSafeLeafName("C:foo")).toBe(false)
    expect(isSafeLeafName(":foo")).toBe(false)
    expect(isSafeLeafName("\\\\server\\share\\a")).toBe(false)
    expect(isSafeLeafName("//server/share/a")).toBe(false)
  })
})

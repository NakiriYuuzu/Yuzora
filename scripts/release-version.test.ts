import { readFileSync } from "node:fs"

import { describe, expect, it } from "vitest"

import { assertReleaseVersion, classifyReleaseVersion, versionFromTag } from "./release-version"
import { verifyVersionConsistency } from "./verify-version-consistency"

describe("release version classification", () => {
  it("accepts only stable and beta release versions", () => {
    expect(classifyReleaseVersion("0.0.9")).toBe("stable")
    expect(classifyReleaseVersion("0.0.9-beta.1")).toBe("beta")
    expect(versionFromTag("v0.0.9-beta.1")).toBe("0.0.9-beta.1")
  })

  it("accepts the current beta product version and matching tag", () => {
    expect(verifyVersionConsistency(process.cwd(), "v0.0.9-beta.3")).toBe(
      "Version consistency verified: v0.0.9-beta.3"
    )
  })

  it("keeps both README version badges aligned with the product version", () => {
    const version = JSON.parse(readFileSync("package.json", "utf8")).version as string
    const badgeVersion = version.replaceAll("-", "--")
    for (const readme of ["README.md", "README.zh-TW.md"]) {
      expect(readFileSync(readme, "utf8")).toContain(
        `img.shields.io/badge/version-${badgeVersion}-`
      )
    }
  })

  it.each(["0.0.9-beta.0", "0.0.9-rc.1", "0.0.9+build.1", "0.0.9-preview.1", "00.0.9"]) (
    "rejects unsupported release version %s",
    (version) => {
      expect(() => assertReleaseVersion(version)).toThrow("must be stable X.Y.Z or beta X.Y.Z-beta.N")
    }
  )
})

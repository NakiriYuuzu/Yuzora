import { mkdtempSync, mkdirSync, readFileSync, rmSync, writeFileSync } from "node:fs"
import { tmpdir } from "node:os"
import { join } from "node:path"

import { describe, expect, it } from "vitest"

import { assertReleaseVersion, classifyReleaseVersion, versionFromTag } from "./release-version"
import { verifyVersionConsistency } from "./verify-version-consistency"

describe("release version classification", () => {
  it("accepts only stable and beta release versions", () => {
    expect(classifyReleaseVersion("0.0.9")).toBe("stable")
    expect(classifyReleaseVersion("0.0.9-beta.1")).toBe("beta")
    expect(versionFromTag("v0.0.9-beta.1")).toBe("0.0.9-beta.1")
  })

  it("accepts the current stable product version and matching tag", () => {
    const version = JSON.parse(readFileSync("package.json", "utf8")).version as string
    expect(verifyVersionConsistency(process.cwd(), `v${version}`)).toBe(
      `Version consistency verified: v${version}`
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

describe("version consistency across checkout line endings", () => {
  const version = "0.0.19"
  function fixture(lineEnding: string, rootVersion = version, hostVersion = version) {
    const root = mkdtempSync(join(tmpdir(), "yuzora-version-check-"))
    mkdirSync(join(root, "src-tauri/host"), { recursive: true })
    const files: Record<string, string> = {
      "package.json": JSON.stringify({ version }),
      "src-tauri/tauri.conf.json": JSON.stringify({ version }),
      "src-tauri/Cargo.toml": `[package]\nname = "yuzora"\nversion = "${version}"\n`,
      "src-tauri/host/Cargo.toml": `[package]\nname = "yuzora-host"\nversion = "${version}"\n`,
      "src-tauri/Cargo.lock": `version = 4\n\n[[package]]\nname = "yuzora"\nversion = "${rootVersion}"\n`,
      "src-tauri/host/Cargo.lock": `version = 4\n\n[[package]]\nname = "yuzora-host"\nversion = "${hostVersion}"\n`,
    }
    for (const [path, content] of Object.entries(files)) {
      writeFileSync(join(root, path), content.replaceAll("\n", lineEnding))
    }
    return root
  }

  it.each(["\n", "\r\n"])("accepts matching versions with %j line endings", (lineEnding) => {
    const root = fixture(lineEnding)
    try {
      expect(verifyVersionConsistency(root, `v${version}`)).toBe(`Version consistency verified: v${version}`)
    } finally {
      rmSync(root, { recursive: true, force: true })
    }
  })

  it.each(["\n", "\r\n"])("still rejects both lockfile mismatches with %j line endings", (lineEnding) => {
    const root = fixture(lineEnding, "0.0.18", "0.0.17")
    try {
      expect(() => verifyVersionConsistency(root, `v${version}`)).toThrow(
        "Cargo.lock root package version 0.0.18 != 0.0.19\nhost Cargo.lock package version 0.0.17 != 0.0.19",
      )
    } finally {
      rmSync(root, { recursive: true, force: true })
    }
  })
})

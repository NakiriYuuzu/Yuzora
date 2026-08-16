import { readFile } from "node:fs/promises"
import { dirname, resolve } from "node:path"
import { fileURLToPath } from "node:url"
import { describe, expect, it } from "vitest"
import {
  HERDR_RESOURCE_TARGETS,
  HERDR_RESOURCE_VERSION,
  resourceTargetIdsForHost,
  validateArchiveEntries
} from "./prepare-herdr-resources"

const repositoryRoot = resolve(dirname(fileURLToPath(import.meta.url)), "..")

describe("prepare Herdr resources", () => {
  it("pins protocol-19 Herdr resources for both released desktop platforms", () => {
    expect(HERDR_RESOURCE_VERSION).toEqual({
      baseVersion: "0.8.0",
      protocol: 19,
      windowsBuildId: "2026-08-04-d78e3d3b5126",
      licenseSha256: "c71d239df91726fc519c6eb72d318ec65820627232b2f796219e87dcf35d0ab4"
    })
    expect(resourceTargetIdsForHost("darwin")).toEqual(["macos-aarch64", "macos-x86_64"])
    expect(resourceTargetIdsForHost("win32")).toEqual(["windows-x86_64"])
    expect(() => resourceTargetIdsForHost("linux")).toThrow(/does not build desktop/)
  })

  it("pins every downloaded archive and extracted file by SHA-256", () => {
    for (const target of Object.values(HERDR_RESOURCE_TARGETS)) {
      expect(target.archiveSha256).toMatch(/^[a-f0-9]{64}$/)
      expect(target.files.length).toBeGreaterThan(0)
      for (const file of target.files) {
        expect(file.path).not.toMatch(/(^|\/)\.\.(\/|$)/)
        expect(file.sha256).toMatch(/^[a-f0-9]{64}$/)
      }
    }
    expect(HERDR_RESOURCE_TARGETS["windows-x86_64"].files.map((file) => file.path)).toContain(
      "conpty/x64/OpenConsole.exe"
    )
  })

  it("maps prepared resources into each released desktop bundle", async () => {
    const macos = JSON.parse(
      await readFile(resolve(repositoryRoot, "src-tauri/tauri.macos.conf.json"), "utf8")
    )
    const windows = JSON.parse(
      await readFile(resolve(repositoryRoot, "src-tauri/tauri.windows.conf.json"), "utf8")
    )
    expect(macos.bundle.resources).toMatchObject({
      "resources/herdr/macos-aarch64/": "herdr/macos-aarch64/",
      "resources/herdr/macos-x86_64/": "herdr/macos-x86_64/"
    })
    expect(windows.bundle.resources).toMatchObject({
      "resources/herdr/windows-x86_64/": "herdr/windows-x86_64/"
    })
  })

  it("accepts only the exact pinned archive shape", () => {
    const expected = HERDR_RESOURCE_TARGETS["windows-x86_64"].files.map((file) => file.path)
    expect(() => validateArchiveEntries([...expected, "conpty/"], expected)).not.toThrow()
    expect(() => validateArchiveEntries([...expected, "unexpected.dll"], expected)).toThrow(
      /archive contents changed/
    )
    expect(() => validateArchiveEntries(["../herdr.exe"], ["herdr.exe"])).toThrow(/unsafe path/)
    expect(() => validateArchiveEntries([String.raw`C:\herdr.exe`], ["herdr.exe"])).toThrow(
      /unsafe path/
    )
  })
})

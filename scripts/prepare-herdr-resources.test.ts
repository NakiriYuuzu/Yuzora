import { readFile } from "node:fs/promises"
import { dirname, resolve } from "node:path"
import { fileURLToPath } from "node:url"
import { afterEach, describe, expect, it, vi } from "vitest"
import {
  fetchWithRetry,
  HERDR_RESOURCE_TARGETS,
  HERDR_RESOURCE_VERSION,
  resourceTargetIdsForHost,
  validateArchiveEntries,
  zipExtractionToolForPlatform
} from "./prepare-herdr-resources"

const repositoryRoot = resolve(dirname(fileURLToPath(import.meta.url)), "..")

afterEach(() => {
  vi.useRealTimers()
  vi.unstubAllGlobals()
})

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
    expect(zipExtractionToolForPlatform("win32")).toBe("powershell")
    expect(zipExtractionToolForPlatform("darwin")).toBe("tar")
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

  it("retries transient download failures with bounded backoff", async () => {
    vi.useFakeTimers()
    const response = new Response("payload", { status: 200 })
    const fetchMock = vi
      .fn()
      .mockRejectedValueOnce(new Error("connection refused"))
      .mockResolvedValueOnce(response)
    vi.stubGlobal("fetch", fetchMock)

    const pending = fetchWithRetry("https://example.invalid/herdr")
    await vi.advanceTimersByTimeAsync(1_000)

    await expect(pending).resolves.toBe(response)
    expect(fetchMock).toHaveBeenCalledTimes(2)
  })

  it("does not retry permanent HTTP failures", async () => {
    const fetchMock = vi.fn().mockResolvedValue(new Response("missing", { status: 404 }))
    vi.stubGlobal("fetch", fetchMock)

    await expect(fetchWithRetry("https://example.invalid/herdr")).rejects.toThrow(/HTTP 404/)
    expect(fetchMock).toHaveBeenCalledTimes(1)
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

import { readFile } from "node:fs/promises"
import { dirname, resolve } from "node:path"
import { fileURLToPath } from "node:url"
import { afterEach, describe, expect, it, vi } from "vitest"
import {
  fetchWithRetry,
  HERDR_RESOURCE_TARGETS,
  HERDR_RESOURCE_VERSION,
  resourceTargetIdsForHost
} from "./prepare-herdr-resources"

const repositoryRoot = resolve(dirname(fileURLToPath(import.meta.url)), "..")

afterEach(() => {
  vi.useRealTimers()
  vi.unstubAllGlobals()
})

describe("prepare Herdr resources", () => {
  it("pins protocol-20 Herdr v0.8.2 Stable resources for both released desktop platforms", () => {
    expect(HERDR_RESOURCE_VERSION).toEqual({
      baseVersion: "0.8.2",
      protocol: 20,
      licenseSha256: "c71d239df91726fc519c6eb72d318ec65820627232b2f796219e87dcf35d0ab4"
    })
    expect(HERDR_RESOURCE_TARGETS["macos-aarch64"].url).toContain("/v0.8.2/")
    expect(HERDR_RESOURCE_TARGETS["macos-x86_64"].url).toContain("/v0.8.2/")
    expect(Object.keys(HERDR_RESOURCE_TARGETS).sort()).toEqual(["linux-aarch64", "linux-x86_64", "macos-aarch64", "macos-x86_64"])
    expect(resourceTargetIdsForHost("darwin")).toEqual(["macos-aarch64", "macos-x86_64"])
    expect(resourceTargetIdsForHost("win32")).toEqual(["linux-aarch64", "linux-x86_64"])
    expect(resourceTargetIdsForHost("linux")).toEqual(["linux-aarch64", "linux-x86_64"])

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
    expect(windows.bundle.resources).toEqual({
      "resources/host/": "host/",
      "resources/legacy-cleanup/": "legacy-cleanup/"
    })
    expect(macos.bundle.resources["resources/host/"]).toBe("host/")
  })
})

import { access, readFile } from "node:fs/promises"
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
  it("pins protocol-20 Herdr v0.8.2 Stable resources for both released desktop platforms", () => {
    expect(HERDR_RESOURCE_VERSION).toEqual({
      baseVersion: "0.8.2",
      protocol: 20,
      licenseSha256: "c71d239df91726fc519c6eb72d318ec65820627232b2f796219e87dcf35d0ab4"
    })
    expect(HERDR_RESOURCE_TARGETS["macos-aarch64"].url).toContain("/v0.8.2/")
    expect(HERDR_RESOURCE_TARGETS["macos-x86_64"].url).toContain("/v0.8.2/")
    expect(HERDR_RESOURCE_TARGETS["windows-x86_64"].url).toBe(
      "https://github.com/herdrdev/herdr/releases/download/v0.8.2/herdr-windows-x86_64.zip"
    )
    expect(HERDR_RESOURCE_TARGETS["windows-x86_64"].url).not.toMatch(/preview-/)
    expect(HERDR_RESOURCE_TARGETS["windows-x86_64"].files.map((file) => file.path).sort()).toEqual(
      [
        "THIRD-PARTY-NOTICES/Microsoft.Windows.Console.ConPTY-LICENSE.txt",
        "THIRD-PARTY-NOTICES/Microsoft.Windows.Console.ConPTY-NOTICE.md",
        "conpty/arm64/OpenConsole.exe",
        "conpty/conpty.dll",
        "conpty/herdr-conpty.json",
        "conpty/x64/OpenConsole.exe",
        "herdr.exe"
      ].sort()
    )
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

    const pluginResources = Object.fromEntries(
      Object.entries(windows.bundle.resources).filter(([, target]) =>
        String(target).startsWith("herdr-plugins/yuzora-wsl-agents/")
      )
    )
    expect(pluginResources).toEqual({
      "../herdr-plugins/yuzora-wsl-agents/herdr-plugin.toml":
        "herdr-plugins/yuzora-wsl-agents/herdr-plugin.toml",
      "../herdr-plugins/yuzora-wsl-agents/README.md":
        "herdr-plugins/yuzora-wsl-agents/README.md",
      "../herdr-plugins/yuzora-wsl-agents/adapters/common/herdr-wsl-report":
        "herdr-plugins/yuzora-wsl-agents/adapters/common/herdr-wsl-report",
      "../herdr-plugins/yuzora-wsl-agents/adapters/install.sh":
        "herdr-plugins/yuzora-wsl-agents/adapters/install.sh",
      "../herdr-plugins/yuzora-wsl-agents/adapters/pi/yuzora-herdr-wsl.ts":
        "herdr-plugins/yuzora-wsl-agents/adapters/pi/yuzora-herdr-wsl.ts",
      "../herdr-plugins/yuzora-wsl-agents/scripts/check-status.ps1":
        "herdr-plugins/yuzora-wsl-agents/scripts/check-status.ps1",
      "../herdr-plugins/yuzora-wsl-agents/scripts/common.ps1":
        "herdr-plugins/yuzora-wsl-agents/scripts/common.ps1",
      "../herdr-plugins/yuzora-wsl-agents/scripts/manage-adapters.ps1":
        "herdr-plugins/yuzora-wsl-agents/scripts/manage-adapters.ps1",
      "../herdr-plugins/yuzora-wsl-agents/scripts/manage-bundled-plugin.ps1":
        "herdr-plugins/yuzora-wsl-agents/scripts/manage-bundled-plugin.ps1",
      "../herdr-plugins/yuzora-wsl-agents/scripts/open-pane.ps1":
        "herdr-plugins/yuzora-wsl-agents/scripts/open-pane.ps1"
    })
    expect(Object.keys(pluginResources).join("\n")).not.toMatch(/\/tests\/|\/lib\//)
    expect(Object.keys(pluginResources)).toHaveLength(10)
    for (const source of Object.keys(pluginResources)) {
      await expect(access(resolve(repositoryRoot, "src-tauri", source))).resolves.toBeUndefined()
    }
  })

  it("verifies the bundled plugin inside both Windows installer formats", async () => {
    const verifier = await readFile(
      resolve(repositoryRoot, "scripts/verify-windows-bundled-wsl-plugin.ps1"),
      "utf8"
    )
    expect(verifier).toContain("msiexec.exe")
    expect(verifier).toContain("7z.exe")
    expect(verifier).toContain("MSI administrative extraction")
    expect(verifier).toContain("NSIS 7-Zip extraction")
    for (const required of [
      "herdr-plugin.toml",
      "README.md",
      "adapters\\common\\herdr-wsl-report",
      "adapters\\install.sh",
      "adapters\\pi\\yuzora-herdr-wsl.ts",
      "scripts\\manage-bundled-plugin.ps1",
      "scripts\\open-pane.ps1"
    ]) {
      expect(verifier).toContain(required)
    }
    expect(verifier).toContain("Compare-Object -ReferenceObject $ExpectedFiles")
    expect(verifier).toContain("Get-FileHash -LiteralPath $sourcePath -Algorithm SHA256")
    expect(verifier).toContain("Get-FileHash -LiteralPath $bundledPath -Algorithm SHA256")
    expect(verifier).toContain("System.Management.Automation.Language.Parser")
    expect(verifier).toContain("[ref]$tokens")
    expect(verifier).toContain("[ref]$parseErrors")
    expect(verifier).toContain("expected Windows PowerShell 5.1")
    expect(verifier).toContain("Invoke-BundledHelper -Payload $msiPayload -Action link")
    expect(verifier).toContain("already registered from another root")
    expect(verifier).toContain("refusing to unlink")
    expect(verifier).toContain("Remove-Item -LiteralPath $tempRoot")
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

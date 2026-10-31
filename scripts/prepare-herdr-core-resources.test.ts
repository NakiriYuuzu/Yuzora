import { spawnSync } from "node:child_process"
import { createHash } from "node:crypto"
import { mkdir, mkdtemp, readFile, readdir, rm, stat, writeFile } from "node:fs/promises"
import { tmpdir } from "node:os"
import { dirname, join, resolve } from "node:path"
import { fileURLToPath } from "node:url"
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"
import {
  HERDR_CORE_ARCHIVE_DIR_ENV,
  HERDR_CORE_MANIFEST,
  archiveDirFromEnv,
  archiveExtractionCommand,
  coreTargetIdsForHost,
  prepareCoreTarget,
  prepareHerdrCoreResources,
  verifyLicenseFiles,
  type HerdrCoreManifest,
  type HerdrCoreResourceTarget
} from "./prepare-herdr-core-resources"

const repositoryRoot = resolve(dirname(fileURLToPath(import.meta.url)), "..")
const sha256 = (bytes: string | Uint8Array) => createHash("sha256").update(bytes).digest("hex")
const MIB = 1024 * 1024

// The pins come from the host's replay fixture alone. A copy kept in this file would be
// compared with a manifest written by the same hand. A missing fixture fails the test:
// `readFile` rejects, and there is no fallback.
interface BundledPins {
  version: string
  generation: number
  tag: string
  source_commit: string
  manifest_url: string
  targets: Record<string, { url: string; archive_sha256: string; files: { path: string; sha256: string }[] }>
}

async function loadBundledPins(): Promise<BundledPins> {
  const fixture = resolve(repositoryRoot, "src-tauri/host/tests/fixtures/herdr-core/pins.json")
  return JSON.parse(await readFile(fixture, "utf8")).bundled
}

const byPath = <T extends { path: string }>(files: T[]) => [...files].sort((a, b) => (a.path < b.path ? -1 : 1))

describe("herdr-core runtime manifest", () => {
  it("matches the pinned bundled version, archives and files in the Rust replay fixture", async () => {
    const pins = await loadBundledPins()
    expect(HERDR_CORE_MANIFEST.version).toBe(pins.version)
    expect(HERDR_CORE_MANIFEST.generation).toBe(pins.generation)
    expect(HERDR_CORE_MANIFEST.sourceTag).toBe(pins.tag)
    expect(HERDR_CORE_MANIFEST.sourceCommit).toBe(pins.source_commit)
    expect(HERDR_CORE_MANIFEST.manifestUrl).toBe(pins.manifest_url)
    expect(Object.keys(HERDR_CORE_MANIFEST.targets).sort()).toEqual(Object.keys(pins.targets).sort())
    for (const [id, pinned] of Object.entries(pins.targets)) {
      const target = HERDR_CORE_MANIFEST.targets[id]
      expect(target.url).toBe(pinned.url)
      expect(target.archiveSha256).toBe(pinned.archive_sha256)
      expect(byPath(target.files)).toEqual(byPath(pinned.files.map(({ path, sha256 }) => ({ path, sha256 }))))
    }
  })

  it("ships only the two release platforms, each from the pinned upstream manifest path", () => {
    expect(Object.keys(HERDR_CORE_MANIFEST.targets).sort()).toEqual(["macos-aarch64", "windows-x86_64"])
    expect(HERDR_CORE_MANIFEST.targets["macos-aarch64"].format).toBe("tar.gz")
    expect(HERDR_CORE_MANIFEST.targets["windows-x86_64"].format).toBe("zip")
    expect(HERDR_CORE_MANIFEST.manifestUrl).toBe(`https://releases.herdr.dev/core/${HERDR_CORE_MANIFEST.version}/manifest.json`)
    for (const [key, target] of Object.entries(HERDR_CORE_MANIFEST.targets)) {
      expect(target.id).toBe(key)
      expect(target.destination).toBe(key)
      expect(target.url.startsWith(`https://releases.herdr.dev/core/${HERDR_CORE_MANIFEST.version}/`)).toBe(true)
    }
  })

  it("pins every archive and extracted file by SHA-256 with confined relative paths", () => {
    for (const target of Object.values(HERDR_CORE_MANIFEST.targets)) {
      expect(target.archiveSha256).toMatch(/^[a-f0-9]{64}$/)
      expect(target.files.length).toBeGreaterThan(0)
      expect(new Set(target.files.map((file) => file.path)).size).toBe(target.files.length)
      for (const file of target.files) {
        expect(file.path).not.toMatch(/(^|\/)\.\.(\/|$)|^\/|\\/)
        expect(file.sha256).toMatch(/^[a-f0-9]{64}$/)
      }
    }
    expect(HERDR_CORE_MANIFEST.targets["macos-aarch64"].files.map((file) => file.path)).toEqual(["herdr-core"])
    expect(HERDR_CORE_MANIFEST.targets["windows-x86_64"].files.map((file) => file.path)).toContain("herdr-core.exe")
  })

  it("records the upstream Apache-2.0 license and third-party notices with their tag source", async () => {
    expect(HERDR_CORE_MANIFEST.licenses.map((license) => license.path)).toEqual([
      "LICENSE-HERDR-CORE.txt",
      "THIRD-PARTY-NOTICES-HERDR-CORE.txt"
    ])
    for (const license of HERDR_CORE_MANIFEST.licenses) {
      expect(license.sha256).toMatch(/^[a-f0-9]{64}$/)
      expect(license.source).toContain(`@${HERDR_CORE_MANIFEST.sourceTag}:`)
    }
    // The repo's own copies must still match the digests the manifest records.
    await expect(verifyLicenseFiles(resolve(repositoryRoot, "src-tauri/resources/herdr-core"))).resolves.toBeUndefined()
  })
})

describe("herdr-core host selection and tooling", () => {
  it("prepares only the target for the build host", () => {
    expect(coreTargetIdsForHost("darwin")).toEqual(["macos-aarch64"])
    expect(coreTargetIdsForHost("win32")).toEqual(["windows-x86_64"])
    expect(() => coreTargetIdsForHost("linux")).toThrow(/linux/)
  })

  it("reads the local archive directory from the environment, ignoring empty values", () => {
    expect(archiveDirFromEnv({ [HERDR_CORE_ARCHIVE_DIR_ENV]: "/tmp/archives" })).toBe("/tmp/archives")
    expect(archiveDirFromEnv({ [HERDR_CORE_ARCHIVE_DIR_ENV]: "" })).toBeUndefined()
    expect(archiveDirFromEnv({})).toBeUndefined()
  })

  it("uses the Windows inbox tar for both formats and system tools elsewhere", () => {
    expect(archiveExtractionCommand("win32", "zip", "D:\\s\\a.zip", "D:\\s\\out", "C:\\Windows")).toEqual(["C:\\Windows\\System32\\tar.exe", "-xf", "D:\\s\\a.zip", "-C", "D:\\s\\out"])
    expect(archiveExtractionCommand("win32", "tar.gz", "D:\\s\\a.tgz", "D:\\s\\out", "C:\\Windows")).toEqual(["C:\\Windows\\System32\\tar.exe", "-xzf", "D:\\s\\a.tgz", "-C", "D:\\s\\out"])
    expect(() => archiveExtractionCommand("win32", "zip", "a.zip", "out")).toThrow("SystemRoot")
    expect(archiveExtractionCommand("darwin", "tar.gz", "/t/a.tgz", "/t/out")).toEqual(["tar", "-xzf", "/t/a.tgz", "-C", "/t/out"])
    expect(archiveExtractionCommand("darwin", "zip", "/t/a.zip", "/t/out")).toEqual(["unzip", "-q", "/t/a.zip", "-d", "/t/out"])
    expect(() => archiveExtractionCommand("darwin", "7z", "/t/a.7z", "/t/out")).toThrow(/Unsupported/)
  })
})

function run(command: string, args: string[], cwd: string) {
  // COPYFILE_DISABLE keeps macOS tar from adding AppleDouble entries.
  const result = spawnSync(command, args, { cwd, encoding: "utf8", env: { ...process.env, COPYFILE_DISABLE: "1" } })
  if (result.status !== 0) throw new Error(`${command} failed: ${result.stderr}`)
}

async function writeTree(root: string, files: Record<string, string>) {
  for (const [path, content] of Object.entries(files)) {
    await mkdir(dirname(join(root, path)), { recursive: true })
    await writeFile(join(root, path), content)
  }
}

describe.skipIf(process.platform === "win32")("prepare herdr-core resources", () => {
  const workdirs: string[] = []
  const fetchMock = vi.fn()

  beforeEach(() => {
    fetchMock.mockReset()
    fetchMock.mockRejectedValue(new Error("tests must not touch the network"))
    vi.stubGlobal("fetch", fetchMock)
    vi.stubEnv(HERDR_CORE_ARCHIVE_DIR_ENV, "")
    vi.spyOn(console, "log").mockImplementation(() => {})
  })

  afterEach(async () => {
    vi.restoreAllMocks()
    vi.unstubAllEnvs()
    vi.unstubAllGlobals()
    await Promise.all(workdirs.splice(0).map((dir) => rm(dir, { recursive: true, force: true })))
  })

  async function sandbox() {
    const dir = await mkdtemp(join(tmpdir(), "yz-hc-prepare-test-"))
    workdirs.push(dir)
    const root = join(dir, "resources")
    const archives = join(dir, "archives")
    await mkdir(root)
    await mkdir(archives)
    return { dir, root, archives }
  }

  // Builds a real archive from `archived`; the manifest pins `declared` files
  // (defaults to the archive contents) so tests can make the two disagree.
  async function fakeTarget(
    dir: string,
    id: string,
    format: "tar.gz" | "zip",
    archived: Record<string, string>,
    declared: Record<string, string> = archived
  ): Promise<{ target: HerdrCoreResourceTarget; archivePath: string }> {
    const staging = join(dir, `src-${id}`)
    await mkdir(staging)
    await writeTree(staging, archived)
    const name = `herdr-core-${id}.${format}`
    const archivePath = join(dir, name)
    if (format === "tar.gz") run("tar", ["-czf", archivePath, "."], staging)
    else run("zip", ["-q", "-r", "-X", archivePath, "."], staging)
    const target: HerdrCoreResourceTarget = {
      id,
      destination: id,
      format,
      url: `https://example.invalid/core/9.9.9/${name}`,
      archiveSha256: sha256(await readFile(archivePath)),
      files: Object.entries(declared).map(([path, content]) => ({ path, sha256: sha256(content) }))
    }
    return { target, archivePath }
  }

  async function copyInto(archivePath: string, archives: string) {
    await writeFile(join(archives, archivePath.split("/").pop()!), await readFile(archivePath))
  }

  async function residue(root: string) {
    return (await readdir(root)).filter((name) => name.startsWith(".prepare-"))
  }

  const unix = { "herdr-core": "fake unix core\n" }
  const windows = {
    "herdr-core.exe": "fake exe\n",
    "conpty/conpty.dll": "fake dll\n",
    "conpty/x64/OpenConsole.exe": "fake console\n",
    "THIRD-PARTY-NOTICES/Microsoft.Windows.Console.ConPTY-NOTICE.md": "notice\n"
  }

  it("extracts a verified tar.gz and zip from the local archive directory without the network", async () => {
    const { dir, root, archives } = await sandbox()
    const mac = await fakeTarget(dir, "macos-aarch64", "tar.gz", unix)
    const win = await fakeTarget(dir, "windows-x86_64", "zip", windows)
    await copyInto(mac.archivePath, archives)
    await copyInto(win.archivePath, archives)

    await prepareCoreTarget(root, mac.target, archives)
    await prepareCoreTarget(root, win.target, archives)

    expect(await readFile(join(root, "macos-aarch64/herdr-core"), "utf8")).toBe(unix["herdr-core"])
    expect(await readFile(join(root, "windows-x86_64/conpty/x64/OpenConsole.exe"), "utf8")).toBe("fake console\n")
    expect(await residue(root)).toEqual([])
    expect(fetchMock).not.toHaveBeenCalled()
  })

  it("rejects an archive with the wrong SHA-256 before extracting anything", async () => {
    const { dir, root, archives } = await sandbox()
    const { target } = await fakeTarget(dir, "macos-aarch64", "tar.gz", unix)
    // Not even a valid archive: a digest check that ran after extraction would
    // report an extraction failure instead.
    await writeFile(join(archives, "herdr-core-macos-aarch64.tar.gz"), "tampered bytes")

    await expect(prepareCoreTarget(root, target, archives)).rejects.toThrow(/digest mismatch for macos-aarch64/)
    expect(await readdir(root)).toEqual([])
  })

  it("rejects an archive containing a file beyond the manifest", async () => {
    const { dir, root, archives } = await sandbox()
    const { target, archivePath } = await fakeTarget(dir, "macos-aarch64", "tar.gz", { ...unix, "extra.bin": "unexpected" }, unix)
    await copyInto(archivePath, archives)

    await expect(prepareCoreTarget(root, target, archives)).rejects.toThrow(/failed file verification/)
    expect(await readdir(root)).toEqual([])
  })

  it("rejects an archive missing a file the manifest requires", async () => {
    const { dir, root, archives } = await sandbox()
    const { target, archivePath } = await fakeTarget(dir, "windows-x86_64", "zip", { "herdr-core.exe": "fake exe\n" }, { "herdr-core.exe": "fake exe\n", "conpty/conpty.dll": "fake dll\n" })
    await copyInto(archivePath, archives)

    await expect(prepareCoreTarget(root, target, archives)).rejects.toThrow(/failed file verification/)
    expect(await readdir(root)).toEqual([])
  })

  it("rejects an extracted file whose SHA-256 differs from the manifest", async () => {
    const { dir, root, archives } = await sandbox()
    const { target, archivePath } = await fakeTarget(dir, "macos-aarch64", "tar.gz", { "herdr-core": "other content\n" }, unix)
    await copyInto(archivePath, archives)

    await expect(prepareCoreTarget(root, target, archives)).rejects.toThrow(/failed file verification/)
    expect(await readdir(root)).toEqual([])
  })

  it("keeps an existing destination when a replacement fails verification", async () => {
    const { dir, root, archives } = await sandbox()
    const { target, archivePath } = await fakeTarget(dir, "macos-aarch64", "tar.gz", { ...unix, "extra.bin": "x" }, unix)
    await copyInto(archivePath, archives)
    await writeTree(join(root, "macos-aarch64"), { "herdr-core": "stale\n" })

    await expect(prepareCoreTarget(root, target, archives)).rejects.toThrow()
    expect(await readFile(join(root, "macos-aarch64/herdr-core"), "utf8")).toBe("stale\n")
    expect(await residue(root)).toEqual([])
  })

  it("skips a destination that already verifies, without needing the archive", async () => {
    const { dir, root, archives } = await sandbox()
    const { target, archivePath } = await fakeTarget(dir, "macos-aarch64", "tar.gz", unix)
    await copyInto(archivePath, archives)
    await prepareCoreTarget(root, target, archives)
    const before = (await stat(join(root, "macos-aarch64/herdr-core"))).mtimeMs

    // Neither an empty archive directory nor the network may be needed now.
    await prepareCoreTarget(root, target, join(dir, "no-archives-here"))
    await prepareCoreTarget(root, target)

    expect((await stat(join(root, "macos-aarch64/herdr-core"))).mtimeMs).toBe(before)
    expect(console.log).toHaveBeenCalledWith("herdr-core resource macos-aarch64 is already verified")
    expect(fetchMock).not.toHaveBeenCalled()
  })

  it("repairs a destination that was modified or gained a stray file", async () => {
    const { dir, root, archives } = await sandbox()
    const { target, archivePath } = await fakeTarget(dir, "macos-aarch64", "tar.gz", unix)
    await copyInto(archivePath, archives)
    await prepareCoreTarget(root, target, archives)

    await writeFile(join(root, "macos-aarch64/herdr-core"), "corrupted")
    await writeFile(join(root, "macos-aarch64/stray"), "leftover")
    await prepareCoreTarget(root, target, archives)

    expect(await readdir(join(root, "macos-aarch64"))).toEqual(["herdr-core"])
    expect(await readFile(join(root, "macos-aarch64/herdr-core"), "utf8")).toBe(unix["herdr-core"])
  })

  it("never falls back to the network when the local archive is missing, empty or oversized", async () => {
    const { dir, root, archives } = await sandbox()
    const { target } = await fakeTarget(dir, "macos-aarch64", "tar.gz", unix)

    await expect(prepareCoreTarget(root, target, archives)).rejects.toThrow(/is missing at/)
    await writeFile(join(archives, "herdr-core-macos-aarch64.tar.gz"), "")
    await expect(prepareCoreTarget(root, target, archives)).rejects.toThrow(/is empty/)
    await writeFile(join(archives, "herdr-core-macos-aarch64.tar.gz"), Buffer.alloc(33 * MIB))
    await expect(prepareCoreTarget(root, target, archives)).rejects.toThrow(/size limit/)

    expect(fetchMock).not.toHaveBeenCalled()
    expect(await readdir(root)).toEqual([])
  })

  it("downloads with a size cap and verifies the digest before extracting", async () => {
    const { dir, root } = await sandbox()
    const { target, archivePath } = await fakeTarget(dir, "macos-aarch64", "tar.gz", unix)

    fetchMock.mockResolvedValueOnce(new Response(await readFile(archivePath)))
    await prepareCoreTarget(root, target)
    expect(fetchMock).toHaveBeenCalledWith(target.url, { redirect: "follow" })
    expect(await readFile(join(root, "macos-aarch64/herdr-core"), "utf8")).toBe(unix["herdr-core"])

    const other = join(dir, "other")
    await mkdir(other)
    fetchMock.mockResolvedValueOnce(new Response("not the pinned archive"))
    await expect(prepareCoreTarget(other, target)).rejects.toThrow(/digest mismatch/)
    expect(await readdir(other)).toEqual([])

    fetchMock.mockResolvedValueOnce(new Response("x", { headers: { "content-length": String(33 * MIB) } }))
    await expect(prepareCoreTarget(other, target)).rejects.toThrow(/download limit/)
    fetchMock.mockResolvedValueOnce(new Response(Buffer.alloc(33 * MIB)))
    await expect(prepareCoreTarget(other, target)).rejects.toThrow(/download limit/)
    fetchMock.mockResolvedValueOnce(new Response(""))
    await expect(prepareCoreTarget(other, target)).rejects.toThrow(/empty/)
    expect(await readdir(other)).toEqual([])
  })

  it("selects the host target from the environment's archive directory and leaves the other alone", async () => {
    const { dir, root, archives } = await sandbox()
    const mac = await fakeTarget(dir, "macos-aarch64", "tar.gz", unix)
    const win = await fakeTarget(dir, "windows-x86_64", "zip", windows)
    await copyInto(mac.archivePath, archives)
    await copyInto(win.archivePath, archives)
    const manifest: HerdrCoreManifest = {
      ...HERDR_CORE_MANIFEST,
      licenses: [],
      targets: { "macos-aarch64": mac.target, "windows-x86_64": win.target }
    }

    vi.stubEnv(HERDR_CORE_ARCHIVE_DIR_ENV, archives)
    await prepareHerdrCoreResources({ platform: "darwin", root, manifest })
    expect(await readdir(root)).toEqual(["macos-aarch64"])

    await prepareHerdrCoreResources({ platform: "win32", root, manifest })
    expect((await readdir(root)).sort()).toEqual(["macos-aarch64", "windows-x86_64"])
    await expect(prepareHerdrCoreResources({ platform: "linux", root, manifest })).rejects.toThrow(/linux/)
    expect(fetchMock).not.toHaveBeenCalled()
  })

  it("refuses to prepare anything when a license file is missing or altered", async () => {
    const { dir, root, archives } = await sandbox()
    const mac = await fakeTarget(dir, "macos-aarch64", "tar.gz", unix)
    await copyInto(mac.archivePath, archives)
    const licenseText = "Apache-2.0 stand-in\n"
    const manifest: HerdrCoreManifest = {
      ...HERDR_CORE_MANIFEST,
      licenses: [{ path: "LICENSE.txt", sha256: sha256(licenseText), source: "test" }],
      targets: { "macos-aarch64": mac.target }
    }

    await expect(prepareHerdrCoreResources({ platform: "darwin", root, archiveDir: archives, manifest })).rejects.toThrow(/license file is missing/)
    await writeFile(join(root, "LICENSE.txt"), "something else\n")
    await expect(prepareHerdrCoreResources({ platform: "darwin", root, archiveDir: archives, manifest })).rejects.toThrow(/license digest mismatch/)
    expect(await readdir(root)).toEqual(["LICENSE.txt"])

    await writeFile(join(root, "LICENSE.txt"), licenseText)
    await prepareHerdrCoreResources({ platform: "darwin", root, archiveDir: archives, manifest })
    expect((await readdir(root)).sort()).toEqual(["LICENSE.txt", "macos-aarch64"])
  })
})

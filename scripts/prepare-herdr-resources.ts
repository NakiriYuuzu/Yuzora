import { createHash } from "node:crypto"
import {
  chmod,
  mkdir,
  mkdtemp,
  readFile,
  readdir,
  rename,
  rm,
  stat,
  writeFile
} from "node:fs/promises"
import { basename, dirname, join, relative, resolve, sep } from "node:path"
import { fileURLToPath } from "node:url"

const MAX_DOWNLOAD_BYTES = 64 * 1024 * 1024
const SCRIPT_DIR = dirname(fileURLToPath(import.meta.url))
const RESOURCE_ROOT = resolve(SCRIPT_DIR, "../src-tauri/resources/herdr")

interface ResourceFile {
  path: string
  sha256: string
}

export interface HerdrResourceTarget {
  id: string
  destination: string
  url: string
  archiveSha256: string
  format: "binary" | "zip"
  files: ResourceFile[]
}

export const HERDR_RESOURCE_VERSION = {
  baseVersion: "0.8.0",
  protocol: 19,
  windowsBuildId: "2026-08-04-d78e3d3b5126",
  licenseSha256: "c71d239df91726fc519c6eb72d318ec65820627232b2f796219e87dcf35d0ab4"
} as const

export const HERDR_RESOURCE_TARGETS: Record<string, HerdrResourceTarget> = {
  "macos-aarch64": {
    id: "macos-aarch64",
    destination: "macos-aarch64",
    url: "https://github.com/herdrdev/herdr/releases/download/v0.8.0/herdr-macos-aarch64",
    archiveSha256: "d53a9f93fccfdfcc55632927bf51002f5add0aa7990bcdf508ffbd84ac658178",
    format: "binary",
    files: [
      {
        path: "herdr",
        sha256: "d53a9f93fccfdfcc55632927bf51002f5add0aa7990bcdf508ffbd84ac658178"
      }
    ]
  },
  "macos-x86_64": {
    id: "macos-x86_64",
    destination: "macos-x86_64",
    url: "https://github.com/herdrdev/herdr/releases/download/v0.8.0/herdr-macos-x86_64",
    archiveSha256: "77cb5afd6c8fcaaaf3bc28e474ec01c209331ad08094e20d7f8aa9b0bb78d649",
    format: "binary",
    files: [
      {
        path: "herdr",
        sha256: "77cb5afd6c8fcaaaf3bc28e474ec01c209331ad08094e20d7f8aa9b0bb78d649"
      }
    ]
  },
  "windows-x86_64": {
    id: "windows-x86_64",
    destination: "windows-x86_64",
    url: "https://github.com/herdrdev/herdr/releases/download/preview-2026-08-04-d78e3d3b5126/herdr-windows-x86_64.zip",
    archiveSha256: "b1d288118848ecd3ef33532a34506edc53a38a416057aee5b7fe1de4188a16fc",
    format: "zip",
    files: [
      {
        path: "herdr.exe",
        sha256: "6f470da358d6713b6bebab922ffb1f5fe1d3d288cc6f374c7dca1b4a9837a542"
      },
      {
        path: "conpty/arm64/OpenConsole.exe",
        sha256: "ed7622fd0d3bedc9ab9f122f5e58edf0def9e7999224f52dd395ba9f54edbe09"
      },
      {
        path: "conpty/x64/OpenConsole.exe",
        sha256: "b7fd936c2668b87b9ecf7b3366dc6568afc1c6f981874cba3e955a1c35cf8160"
      },
      {
        path: "conpty/conpty.dll",
        sha256: "39fba2713e2495117b1591ae8c32a3b904bea7aa66069cf7815e2844c76d75d8"
      },
      {
        path: "conpty/herdr-conpty.json",
        sha256: "c8f499ad82c568e737d6bc7d0b583e3785d2f43af3d2c0cebb856076690533f5"
      },
      {
        path: "THIRD-PARTY-NOTICES/Microsoft.Windows.Console.ConPTY-LICENSE.txt",
        sha256: "5d177f23ecfeb0ea8e050b6a5a16355e1ae9a0b286436ca8f83ed08b3795be6b"
      },
      {
        path: "THIRD-PARTY-NOTICES/Microsoft.Windows.Console.ConPTY-NOTICE.md",
        sha256: "e7fbaadee6ab20c28b87730a510ee5f5815d8fb4bd88d1d54d282dc2a74c0726"
      }
    ]
  }
}

export function resourceTargetIdsForHost(platform: NodeJS.Platform): string[] {
  if (platform === "darwin") return ["macos-aarch64", "macos-x86_64"]
  if (platform === "win32") return ["windows-x86_64"]
  throw new Error(`Yuzora does not build desktop Herdr resources on ${platform}`)
}

export function validateArchiveEntries(entries: string[], expectedFiles: string[]): void {
  const files = entries
    .map((entry) => entry.replaceAll("\\", "/").replace(/^\.\//, ""))
    .filter((entry) => entry.length > 0 && !entry.endsWith("/"))
  for (const entry of files) {
    if (
      entry.startsWith("/") ||
      /^[A-Za-z]:\//.test(entry) ||
      entry.split("/").some((part) => part === "..")
    ) {
      throw new Error(`Herdr archive contains an unsafe path: ${entry}`)
    }
  }
  const actual = [...new Set(files)].sort()
  const expected = [...expectedFiles].sort()
  if (JSON.stringify(actual) !== JSON.stringify(expected)) {
    throw new Error(
      `Herdr archive contents changed: expected ${expected.join(", ")}; received ${actual.join(", ")}`
    )
  }
}

function sha256(bytes: Uint8Array): string {
  return createHash("sha256").update(bytes).digest("hex")
}

async function sha256File(path: string): Promise<string> {
  return sha256(await readFile(path))
}

async function listFiles(root: string, current = root): Promise<string[]> {
  const entries = await readdir(current, { withFileTypes: true })
  const files: string[] = []
  for (const entry of entries) {
    const path = join(current, entry.name)
    if (entry.isDirectory()) files.push(...(await listFiles(root, path)))
    else if (entry.isFile()) files.push(relative(root, path).split(sep).join("/"))
    else throw new Error(`Herdr resource contains an unsupported filesystem entry: ${path}`)
  }
  return files.sort()
}

async function targetIsValid(root: string, target: HerdrResourceTarget): Promise<boolean> {
  const destination = join(root, target.destination)
  try {
    const actualFiles = await listFiles(destination)
    const expectedFiles = target.files.map((file) => file.path).sort()
    if (JSON.stringify(actualFiles) !== JSON.stringify(expectedFiles)) return false
    for (const file of target.files) {
      if ((await sha256File(join(destination, file.path))) !== file.sha256) return false
    }
    return true
  } catch {
    return false
  }
}

export async function fetchWithRetry(url: string): Promise<Response> {
  const attempts = 4
  let lastError: unknown
  for (let attempt = 1; attempt <= attempts; attempt += 1) {
    let response: Response
    try {
      response = await fetch(url, { redirect: "follow" })
    } catch (error) {
      lastError = error
      if (attempt < attempts) {
        await new Promise((resolveDelay) => setTimeout(resolveDelay, attempt * 1_000))
        continue
      }
      break
    }

    if (response.ok) return response
    await response.body?.cancel()
    const error = new Error(`Herdr resource download failed with HTTP ${response.status}: ${url}`)
    if (response.status < 500 && response.status !== 429) throw error
    lastError = error
    if (attempt < attempts) {
      await new Promise((resolveDelay) => setTimeout(resolveDelay, attempt * 1_000))
    }
  }
  throw new Error(`Herdr resource download failed after ${attempts} attempts: ${url}`, {
    cause: lastError
  })
}

async function download(target: HerdrResourceTarget): Promise<Uint8Array> {
  const response = await fetchWithRetry(target.url)
  const declaredLength = Number(response.headers.get("content-length") ?? 0)
  if (declaredLength > MAX_DOWNLOAD_BYTES) {
    throw new Error(`Herdr resource exceeds the ${MAX_DOWNLOAD_BYTES}-byte download limit`)
  }
  if (!response.body) throw new Error(`Herdr resource response has no body: ${target.url}`)
  const chunks: Uint8Array[] = []
  let total = 0
  for await (const chunk of response.body) {
    total += chunk.byteLength
    if (total > MAX_DOWNLOAD_BYTES) {
      throw new Error(`Herdr resource exceeds the ${MAX_DOWNLOAD_BYTES}-byte download limit`)
    }
    chunks.push(chunk)
  }
  if (total === 0) throw new Error("Herdr resource download was empty")
  const bytes = new Uint8Array(total)
  let offset = 0
  for (const chunk of chunks) {
    bytes.set(chunk, offset)
    offset += chunk.byteLength
  }
  const digest = sha256(bytes)
  if (digest !== target.archiveSha256) {
    throw new Error(`Herdr resource digest mismatch for ${target.id}: ${digest}`)
  }
  return bytes
}

function runTar(args: string[], cwd: string): string {
  const result = Bun.spawnSync(["tar", ...args], { cwd, stdout: "pipe", stderr: "pipe" })
  if (result.exitCode !== 0) {
    throw new Error(`tar ${args[0]} failed: ${result.stderr.toString().trim()}`)
  }
  return result.stdout.toString()
}

function runWindowsPowerShell(script: string, archivePath: string, destination?: string): string {
  const result = Bun.spawnSync(
    ["powershell.exe", "-NoLogo", "-NoProfile", "-NonInteractive", "-Command", script],
    {
      env: {
        ...process.env,
        YUZORA_HERDR_ARCHIVE: archivePath,
        YUZORA_HERDR_DESTINATION: destination ?? ""
      },
      stdout: "pipe",
      stderr: "pipe"
    }
  )
  if (result.exitCode !== 0) {
    throw new Error(`PowerShell ZIP operation failed: ${result.stderr.toString().trim()}`)
  }
  return result.stdout.toString()
}

export function zipExtractionToolForPlatform(
  platform: NodeJS.Platform
): "powershell" | "tar" {
  return platform === "win32" ? "powershell" : "tar"
}

function listZipEntries(archivePath: string, stagingRoot: string): string[] {
  if (zipExtractionToolForPlatform(process.platform) === "powershell") {
    const script = `
Add-Type -AssemblyName System.IO.Compression.FileSystem
$archive = [System.IO.Compression.ZipFile]::OpenRead($env:YUZORA_HERDR_ARCHIVE)
try {
  foreach ($entry in $archive.Entries) { [Console]::Out.WriteLine($entry.FullName) }
} finally {
  $archive.Dispose()
}
`
    return runWindowsPowerShell(script, archivePath).split(/\r?\n/)
  }
  return runTar(["-tf", basename(archivePath)], stagingRoot).split(/\r?\n/)
}

function extractZip(archivePath: string, stagingRoot: string, destination: string): void {
  if (zipExtractionToolForPlatform(process.platform) === "powershell") {
    const script = `
Add-Type -AssemblyName System.IO.Compression.FileSystem
[System.IO.Compression.ZipFile]::ExtractToDirectory(
  $env:YUZORA_HERDR_ARCHIVE,
  $env:YUZORA_HERDR_DESTINATION
)
`
    runWindowsPowerShell(script, archivePath, join(stagingRoot, destination))
    return
  }
  runTar(["-xf", basename(archivePath), "-C", destination], stagingRoot)
}

async function prepareTarget(root: string, target: HerdrResourceTarget): Promise<void> {
  if (await targetIsValid(root, target)) {
    console.log(`Herdr resource ${target.id} is already verified`)
    return
  }

  await mkdir(root, { recursive: true })
  const stagingRoot = await mkdtemp(join(root, ".prepare-"))
  const stagingTarget = join(stagingRoot, target.destination)
  const archivePath = join(stagingRoot, basename(new URL(target.url).pathname))
  try {
    const bytes = await download(target)
    await mkdir(stagingTarget, { recursive: true })
    if (target.format === "binary") {
      const output = join(stagingTarget, target.files[0].path)
      await mkdir(dirname(output), { recursive: true })
      await writeFile(output, bytes)
      await chmod(output, 0o755)
    } else {
      await writeFile(archivePath, bytes)
      const entries = listZipEntries(archivePath, stagingRoot)
      validateArchiveEntries(
        entries,
        target.files.map((file) => file.path)
      )
      extractZip(archivePath, stagingRoot, target.destination)
    }

    if (!(await targetIsValid(stagingRoot, target))) {
      throw new Error(`prepared Herdr resource ${target.id} failed file verification`)
    }
    const destination = join(root, target.destination)
    await rm(destination, { recursive: true, force: true })
    await rename(stagingTarget, destination)
    console.log(`Prepared verified Herdr resource ${target.id}`)
  } finally {
    await rm(stagingRoot, { recursive: true, force: true })
  }
}

export async function prepareHerdrResources(
  platform: NodeJS.Platform = process.platform,
  root = RESOURCE_ROOT
): Promise<void> {
  const licensePath = join(root, "LICENSE-HERDR.txt")
  const license = await stat(licensePath).catch(() => null)
  if (!license?.isFile()) {
    throw new Error(`Herdr license file is missing at ${licensePath}`)
  }
  if ((await sha256File(licensePath)) !== HERDR_RESOURCE_VERSION.licenseSha256) {
    throw new Error(`Herdr license digest mismatch at ${licensePath}`)
  }
  for (const id of resourceTargetIdsForHost(platform)) {
    const target = HERDR_RESOURCE_TARGETS[id]
    if (!target) throw new Error(`Herdr resource target ${id} is not configured`)
    await prepareTarget(root, target)
  }
}

if (import.meta.main) {
  await prepareHerdrResources()
}

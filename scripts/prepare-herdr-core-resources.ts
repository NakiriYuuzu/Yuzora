import runtimeManifest from "../src-tauri/herdr-core-runtime.json"
import { fetchWithRetry } from "./prepare-herdr-resources"
import { spawn } from "node:child_process"
import { createHash } from "node:crypto"
import { mkdir, mkdtemp, readFile, readdir, rename, rm, stat, writeFile } from "node:fs/promises"
import { dirname, join, relative, resolve, sep, posix, win32 } from "node:path"
import { fileURLToPath } from "node:url"

const MAX_DOWNLOAD_BYTES = 32 * 1024 * 1024
const SCRIPT_DIR = dirname(fileURLToPath(import.meta.url))
const RESOURCE_ROOT = resolve(SCRIPT_DIR, "../src-tauri/resources/herdr-core")

/** Directory of already downloaded archives; skips the network but not the digest checks. */
export const HERDR_CORE_ARCHIVE_DIR_ENV = "YUZORA_HERDR_CORE_ARCHIVE_DIR"

interface ResourceFile {
  path: string
  sha256: string
}

export interface HerdrCoreLicense extends ResourceFile {
  source: string
}

export interface HerdrCoreResourceTarget {
  id: string
  destination: string
  format: string
  url: string
  archiveSha256: string
  files: ResourceFile[]
}

export interface HerdrCoreManifest {
  version: string
  generation: number
  sourceTag: string
  sourceCommit: string
  manifestUrl: string
  licenses: HerdrCoreLicense[]
  targets: Record<string, HerdrCoreResourceTarget>
}

export const HERDR_CORE_MANIFEST: HerdrCoreManifest = runtimeManifest

export function coreTargetIdsForHost(platform: NodeJS.Platform): string[] {
  // Release platforms only: Apple Silicon macOS and Windows x64. Linux ships no bundled core.
  if (platform === "darwin") return ["macos-aarch64"]
  if (platform === "win32") return ["windows-x86_64"]
  throw new Error(`Yuzora does not bundle herdr-core resources on ${platform}`)
}

export function archiveDirFromEnv(env: NodeJS.ProcessEnv): string | undefined {
  return env[HERDR_CORE_ARCHIVE_DIR_ENV] || undefined
}

export function archiveExtractionCommand(
  platform: NodeJS.Platform,
  format: string,
  archive: string,
  destination: string,
  systemRoot?: string
): string[] {
  if (format !== "tar.gz" && format !== "zip") {
    throw new Error(`Unsupported herdr-core archive format: ${format}`)
  }
  if (platform === "win32") {
    if (!systemRoot || !win32.isAbsolute(systemRoot)) throw new Error("Windows SystemRoot must identify the system archive tool")
    // Git Bash can put GNU tar first on PATH; it treats a drive colon as a remote
    // archive and does not support zip. Use the Windows inbox bsdtar.
    return [win32.join(systemRoot, "System32", "tar.exe"), format === "zip" ? "-xf" : "-xzf", archive, "-C", destination]
  }
  return format === "zip" ? ["unzip", "-q", archive, "-d", destination] : ["tar", "-xzf", archive, "-C", destination]
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
    else throw new Error(`herdr-core resource contains an unsupported filesystem entry: ${path}`)
  }
  return files.sort()
}

async function targetIsValid(root: string, target: HerdrCoreResourceTarget): Promise<boolean> {
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

export async function verifyLicenseFiles(
  root: string,
  licenses: HerdrCoreLicense[] = HERDR_CORE_MANIFEST.licenses
): Promise<void> {
  for (const license of licenses) {
    const path = join(root, license.path)
    const entry = await stat(path).catch(() => null)
    if (!entry?.isFile()) throw new Error(`herdr-core license file is missing at ${path}`)
    if ((await sha256File(path)) !== license.sha256) {
      throw new Error(`herdr-core license digest mismatch at ${path}`)
    }
  }
}

function verifyArchiveDigest(target: HerdrCoreResourceTarget, bytes: Uint8Array): void {
  const digest = sha256(bytes)
  if (digest !== target.archiveSha256) {
    throw new Error(`herdr-core archive digest mismatch for ${target.id}: ${digest}`)
  }
}

async function download(target: HerdrCoreResourceTarget): Promise<Uint8Array> {
  const response = await fetchWithRetry(target.url)
  const declaredLength = Number(response.headers.get("content-length") ?? 0)
  if (declaredLength > MAX_DOWNLOAD_BYTES) {
    throw new Error(`herdr-core archive exceeds the ${MAX_DOWNLOAD_BYTES}-byte download limit`)
  }
  if (!response.body) throw new Error(`herdr-core archive response has no body: ${target.url}`)
  const chunks: Uint8Array[] = []
  let total = 0
  for await (const chunk of response.body) {
    total += chunk.byteLength
    if (total > MAX_DOWNLOAD_BYTES) {
      throw new Error(`herdr-core archive exceeds the ${MAX_DOWNLOAD_BYTES}-byte download limit`)
    }
    chunks.push(chunk)
  }
  if (total === 0) throw new Error("herdr-core archive download was empty")
  const bytes = new Uint8Array(total)
  let offset = 0
  for (const chunk of chunks) {
    bytes.set(chunk, offset)
    offset += chunk.byteLength
  }
  return bytes
}

async function readLocalArchive(target: HerdrCoreResourceTarget, archiveDir: string): Promise<Uint8Array> {
  // An explicit override never falls back to the network.
  const name = posix.basename(new URL(target.url).pathname)
  const path = join(archiveDir, name)
  const entry = await stat(path).catch(() => null)
  if (!entry?.isFile()) throw new Error(`herdr-core archive for ${target.id} is missing at ${path}`)
  if (entry.size > MAX_DOWNLOAD_BYTES) {
    throw new Error(`herdr-core archive exceeds the ${MAX_DOWNLOAD_BYTES}-byte size limit: ${path}`)
  }
  if (entry.size === 0) throw new Error(`herdr-core archive is empty: ${path}`)
  return readFile(path)
}

function extract(command: string[]): Promise<void> {
  return new Promise((resolveExtract, reject) => {
    const child = spawn(command[0], command.slice(1), { stdio: ["ignore", "inherit", "inherit"] })
    child.once("error", reject)
    child.once("close", (code) => {
      if (code === 0) resolveExtract()
      else reject(new Error(`herdr-core archive extraction failed (exit ${code}): ${command[0]}`))
    })
  })
}

export async function prepareCoreTarget(
  root: string,
  target: HerdrCoreResourceTarget,
  archiveDir?: string
): Promise<void> {
  if (await targetIsValid(root, target)) {
    console.log(`herdr-core resource ${target.id} is already verified`)
    return
  }

  await mkdir(root, { recursive: true })
  const stagingRoot = await mkdtemp(join(root, ".prepare-"))
  const stagingTarget = join(stagingRoot, target.destination)
  try {
    const bytes = archiveDir ? await readLocalArchive(target, archiveDir) : await download(target)
    // Only unpack an archive after its pinned upstream digest has matched.
    verifyArchiveDigest(target, bytes)
    const archive = join(stagingRoot, target.format === "zip" ? "archive.zip" : "archive.tar.gz")
    await writeFile(archive, bytes)
    await mkdir(stagingTarget, { recursive: true })
    await extract(archiveExtractionCommand(process.platform, target.format, archive, stagingTarget, process.env.SystemRoot))

    if (!(await targetIsValid(stagingRoot, target))) {
      throw new Error(`prepared herdr-core resource ${target.id} failed file verification`)
    }
    const destination = join(root, target.destination)
    await rm(destination, { recursive: true, force: true })
    await rename(stagingTarget, destination)
    console.log(`Prepared verified herdr-core resource ${target.id}`)
  } finally {
    await rm(stagingRoot, { recursive: true, force: true })
  }
}

export interface PrepareHerdrCoreOptions {
  platform?: NodeJS.Platform
  root?: string
  archiveDir?: string
  manifest?: HerdrCoreManifest
}

export async function prepareHerdrCoreResources({
  platform = process.platform,
  root = RESOURCE_ROOT,
  archiveDir = archiveDirFromEnv(process.env),
  manifest = HERDR_CORE_MANIFEST
}: PrepareHerdrCoreOptions = {}): Promise<void> {
  await verifyLicenseFiles(root, manifest.licenses)
  for (const id of coreTargetIdsForHost(platform)) {
    const target = manifest.targets[id]
    if (!target) throw new Error(`herdr-core resource target ${id} is not configured`)
    await prepareCoreTarget(root, target, archiveDir)
  }
}

if (import.meta.main) {
  await prepareHerdrCoreResources()
}

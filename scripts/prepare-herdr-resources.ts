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
import { dirname, join, relative, resolve, sep } from "node:path"
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
  files: ResourceFile[]
}

export const HERDR_RESOURCE_VERSION = {
  baseVersion: "0.8.2",
  protocol: 20,
  licenseSha256: "c71d239df91726fc519c6eb72d318ec65820627232b2f796219e87dcf35d0ab4"
} as const

export const HERDR_RESOURCE_TARGETS: Record<string, HerdrResourceTarget> = {
  "linux-aarch64": {
    id: "linux-aarch64",
    destination: "linux-aarch64",
    url: "https://github.com/herdrdev/herdr/releases/download/v0.8.2/herdr-linux-aarch64",
    archiveSha256: "f55610658e1c2e0d2aaef730b4b2ab885f7f8ba00285ab372bfb14f2e3d5b40d",
    files: [{ path: "herdr", sha256: "f55610658e1c2e0d2aaef730b4b2ab885f7f8ba00285ab372bfb14f2e3d5b40d" }]
  },
  "linux-x86_64": {
    id: "linux-x86_64",
    destination: "linux-x86_64",
    url: "https://github.com/herdrdev/herdr/releases/download/v0.8.2/herdr-linux-x86_64",
    archiveSha256: "976150a14d490c94b243ea2e1a7eb2dfb67f12e36b182db90936f6728e6aecf4",
    files: [{ path: "herdr", sha256: "976150a14d490c94b243ea2e1a7eb2dfb67f12e36b182db90936f6728e6aecf4" }]
  },
  "macos-aarch64": {
    id: "macos-aarch64",
    destination: "macos-aarch64",
    url: "https://github.com/herdrdev/herdr/releases/download/v0.8.2/herdr-macos-aarch64",
    archiveSha256: "a5d4f4d504d8b309c91f811050559300faba31258425f53c50852fc96f6ae574",
    files: [
      {
        path: "herdr",
        sha256: "a5d4f4d504d8b309c91f811050559300faba31258425f53c50852fc96f6ae574"
      }
    ]
  },
  "macos-x86_64": {
    id: "macos-x86_64",
    destination: "macos-x86_64",
    url: "https://github.com/herdrdev/herdr/releases/download/v0.8.2/herdr-macos-x86_64",
    archiveSha256: "ab50262c8190cd7aa9056d249d255c08c328c3e8716de9cfa29db4f131b8e2c1",
    files: [
      {
        path: "herdr",
        sha256: "ab50262c8190cd7aa9056d249d255c08c328c3e8716de9cfa29db4f131b8e2c1"
      }
    ]
  }
}

export function resourceTargetIdsForHost(platform: NodeJS.Platform): string[] {
  if (platform === "darwin") return ["macos-aarch64", "macos-x86_64"]
  if (platform === "win32" || platform === "linux") return ["linux-aarch64", "linux-x86_64"]
  throw new Error(`Yuzora does not build desktop Herdr resources on ${platform}`)
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
    if (entry.name === ".gitkeep") continue
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

export async function prepareTarget(root: string, target: HerdrResourceTarget): Promise<void> {
  if (await targetIsValid(root, target)) {
    console.log(`Herdr resource ${target.id} is already verified`)
    return
  }

  await mkdir(root, { recursive: true })
  const stagingRoot = await mkdtemp(join(root, ".prepare-"))
  const stagingTarget = join(stagingRoot, target.destination)
  try {
    const bytes = await download(target)
    await mkdir(stagingTarget, { recursive: true })
    const output = join(stagingTarget, target.files[0].path)
    await mkdir(dirname(output), { recursive: true })
    await writeFile(output, bytes)
    await chmod(output, 0o755)

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

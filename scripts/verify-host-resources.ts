import { createHash } from "node:crypto"
import { lstat, readFile, readdir } from "node:fs/promises"
import { dirname, resolve, relative, sep } from "node:path"
import { fileURLToPath } from "node:url"
import { HOST_TARGETS, validateHostArtifact, type HostArtifact } from "./prepare-host-resources"
import { HERDR_RESOURCE_VERSION } from "./prepare-herdr-resources"

const repo = resolve(dirname(fileURLToPath(import.meta.url)), "..")

async function regularFile(path: string): Promise<Buffer> {
  const stat = await lstat(path)
  if (!stat.isFile() || stat.size > 64 * 1024 * 1024) throw new Error(`Invalid runtime resource: ${path}`)
  return readFile(path)
}

async function filesIn(root: string, current = root): Promise<string[]> {
  const entries = await readdir(current, { withFileTypes: true })
  const files: string[] = []
  for (const entry of entries) {
    const path = resolve(current, entry.name)
    if (entry.isDirectory()) files.push(...await filesIn(root, path))
    else if (entry.isFile()) files.push(relative(root, path).split(sep).join("/"))
    else throw new Error(`Invalid runtime resource: ${path}`)
  }
  return files.sort()
}

/** Check all deployment payloads, including the complete Windows ConPTY package. */
export async function verifyHostResources(root: string, version: string): Promise<void> {
  const targets = Object.keys(HOST_TARGETS)
  const expected = ["LICENSE-HERDR.txt", ...targets.flatMap((target) => [target, `${target}.json`])].sort()
  const actual = (await readdir(root)).sort()
  if (JSON.stringify(actual) !== JSON.stringify(expected)) throw new Error("Runtime resources require exactly five targets and their manifests; run host:prepare on the target runners first")
  const license = await regularFile(resolve(root, "LICENSE-HERDR.txt"))
  if (createHash("sha256").update(license).digest("hex") !== HERDR_RESOURCE_VERSION.licenseSha256) throw new Error("HERDR license hash mismatch")
  for (const target of targets) {
    const artifact = JSON.parse((await regularFile(resolve(root, `${target}.json`))).toString()) as HostArtifact
    validateHostArtifact(artifact)
    if (artifact.target !== target || artifact.version !== version) throw new Error(`Runtime version/target mismatch: ${target}`)
    const dir = resolve(root, target)
    const entries = [artifact.helper, artifact.herdr, ...artifact.files ?? []]
    if (!(await lstat(dir)).isDirectory() || JSON.stringify(await filesIn(dir)) !== JSON.stringify(entries.map(entry => entry.path.slice(target.length + 1)).sort())) throw new Error(`Invalid runtime directory: ${target}`)
    for (const entry of entries) {
      const bytes = await regularFile(resolve(root, entry.path))
      if (createHash("sha256").update(bytes).digest("hex") !== entry.sha256) throw new Error(`Runtime hash mismatch: ${entry.path}`)
      const magic = bytes.subarray(0, 4).toString("hex")
      if (entry !== artifact.helper && entry !== artifact.herdr) continue
      if (target.startsWith("windows-") ? !magic.startsWith("4d5a") : target.startsWith("linux-") ? magic !== "7f454c46" : !["cffaedfe", "feedfacf", "cafebabe", "bebafeca"].includes(magic)) throw new Error(`Wrong executable format: ${entry.path}`)
    }
  }
}

if (import.meta.main) {
  const { version } = JSON.parse(await readFile(resolve(repo, "src-tauri/tauri.conf.json"), "utf8")) as { version: string }
  await verifyHostResources(process.argv[2] ?? resolve(repo, "src-tauri/resources/host"), version)
  console.log(`Verified all five host runtime payloads for ${version}`)
}

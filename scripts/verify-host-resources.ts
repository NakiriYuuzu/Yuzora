import { createHash } from "node:crypto"
import { lstat, readFile, readdir } from "node:fs/promises"
import { dirname, resolve } from "node:path"
import { fileURLToPath } from "node:url"
import { HOST_TARGETS, validateHostArtifact, type HostArtifact } from "./prepare-host-resources"
import { HERDR_RESOURCE_VERSION } from "./prepare-herdr-resources"

const repo = resolve(dirname(fileURLToPath(import.meta.url)), "..")

async function regularFile(path: string): Promise<Buffer> {
  const stat = await lstat(path)
  if (!stat.isFile() || stat.size > 64 * 1024 * 1024) throw new Error(`Invalid runtime resource: ${path}`)
  return readFile(path)
}

/** Check deployment payloads before packaging, including resources from all four runners. */
export async function verifyHostResources(root: string, version: string): Promise<void> {
  const targets = Object.keys(HOST_TARGETS)
  const expected = ["LICENSE-HERDR.txt", ...targets.flatMap((target) => [target, `${target}.json`])].sort()
  const actual = (await readdir(root)).sort()
  if (JSON.stringify(actual) !== JSON.stringify(expected)) throw new Error("Runtime resources require exactly four Unix targets and their manifests; run host:prepare on the target runners first")
  const license = await regularFile(resolve(root, "LICENSE-HERDR.txt"))
  if (createHash("sha256").update(license).digest("hex") !== HERDR_RESOURCE_VERSION.licenseSha256) throw new Error("HERDR license hash mismatch")
  for (const target of targets) {
    const artifact = JSON.parse((await regularFile(resolve(root, `${target}.json`))).toString()) as HostArtifact
    validateHostArtifact(artifact)
    if (artifact.target !== target || artifact.version !== version) throw new Error(`Runtime version/target mismatch: ${target}`)
    const dir = resolve(root, target)
    if (!(await lstat(dir)).isDirectory() || JSON.stringify((await readdir(dir)).sort()) !== JSON.stringify(["herdr", "yuzora-host"])) throw new Error(`Invalid runtime directory: ${target}`)
    for (const entry of [artifact.helper, artifact.herdr]) {
      const bytes = await regularFile(resolve(root, entry.path))
      if (createHash("sha256").update(bytes).digest("hex") !== entry.sha256) throw new Error(`Runtime hash mismatch: ${entry.path}`)
      const magic = bytes.subarray(0, 4).toString("hex")
      if (target.startsWith("linux-") ? magic !== "7f454c46" : !["cffaedfe", "feedfacf", "cafebabe", "bebafeca"].includes(magic)) throw new Error(`Not a Unix executable: ${entry.path}`)
    }
  }
}

if (import.meta.main) {
  const { version } = JSON.parse(await readFile(resolve(repo, "src-tauri/tauri.conf.json"), "utf8")) as { version: string }
  await verifyHostResources(process.argv[2] ?? resolve(repo, "src-tauri/resources/host"), version)
  console.log(`Verified all four Unix runtime payloads for ${version}`)
}

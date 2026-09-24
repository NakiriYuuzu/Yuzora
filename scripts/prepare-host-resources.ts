import { createHash } from "node:crypto"
import { chmod, copyFile, mkdir, readFile, rename, writeFile } from "node:fs/promises"
import { dirname, resolve } from "node:path"
import { fileURLToPath } from "node:url"
import { HERDR_RESOURCE_TARGETS, HERDR_RESOURCE_VERSION, prepareTarget } from "./prepare-herdr-resources"

const repo = resolve(dirname(fileURLToPath(import.meta.url)), "..")
export const HOST_TARGETS = {
  "macos-aarch64": "aarch64-apple-darwin",
  "macos-x86_64": "x86_64-apple-darwin",
  "linux-aarch64": "aarch64-unknown-linux-gnu",
  "linux-x86_64": "x86_64-unknown-linux-gnu",
  "windows-x86_64": "x86_64-pc-windows-msvc"
} as const

export interface HostArtifact {
  protocol: 1
  version: string
  target: keyof typeof HOST_TARGETS
  helper: { path: string; sha256: string }
  herdr: { path: string; sha256: string; version: string; protocol: number }
  files?: { path: string; sha256: string }[]
}

export function validateHostArtifact(artifact: HostArtifact): void {
  if (artifact.protocol !== 1 || !Object.hasOwn(HOST_TARGETS, artifact.target)) throw new Error("Unsupported host artifact")
  if (!/^[0-9]+\.[0-9]+\.[0-9]+(?:-[a-zA-Z0-9.-]+)?$/.test(artifact.version)) throw new Error("Invalid host version")
  const suffix = artifact.target.startsWith("windows-") ? ".exe" : ""
  if (artifact.helper.path !== `${artifact.target}/yuzora-host${suffix}` || artifact.herdr.path !== `${artifact.target}/herdr${suffix}`) throw new Error("Invalid artifact path")
  if (![artifact.helper.sha256, artifact.herdr.sha256].every((hash) => /^[a-f0-9]{64}$/.test(hash))) throw new Error("Invalid artifact hash")
  const resources = HERDR_RESOURCE_TARGETS[artifact.target].files
  const official = resources.find(file => file.path === `herdr${suffix}`)!.sha256
  if (artifact.herdr.sha256 !== official || artifact.herdr.version !== HERDR_RESOURCE_VERSION.baseVersion || artifact.herdr.protocol !== HERDR_RESOURCE_VERSION.protocol) throw new Error("Unexpected official HERDR artifact")
  const expected = resources.filter(file => file.path !== `herdr${suffix}`).map(file => ({ path: `${artifact.target}/${file.path}`, sha256: file.sha256 })).sort((a, b) => a.path.localeCompare(b.path))
  const actual = [...artifact.files ?? []].sort((a, b) => a.path.localeCompare(b.path))
  if (JSON.stringify(actual) !== JSON.stringify(expected)) throw new Error("Invalid runtime auxiliary files")
}

export async function prepareHostResources(target: keyof typeof HOST_TARGETS, builder: "build" | "zigbuild" = "build"): Promise<HostArtifact> {
  if (!Object.hasOwn(HOST_TARGETS, target)) throw new Error(`Unsupported target: ${target}`)
  const triple = HOST_TARGETS[target]
  const root = resolve(repo, "src-tauri/resources/host")
  const resource = HERDR_RESOURCE_TARGETS[target]
  const suffix = target.startsWith("windows-") ? ".exe" : ""
  const helperName = `yuzora-host${suffix}`
  const herdrName = `herdr${suffix}`
  const herdrRoot = resolve(repo, "src-tauri/resources/herdr")
  await prepareTarget(herdrRoot, resource)
  const build = Bun.spawn(["cargo", builder, "--locked", "--release", "--target", triple, "--manifest-path", resolve(repo, "src-tauri/host/Cargo.toml")], { cwd: repo, stdout: "inherit", stderr: "inherit" })
  if (await build.exited !== 0) throw new Error(`Helper build failed for ${target}`)
  const version = (JSON.parse(await readFile(resolve(repo, "src-tauri/tauri.conf.json"), "utf8")) as { version: string }).version
  const cargo = await readFile(resolve(repo, "src-tauri/host/Cargo.toml"), "utf8")
  if (!cargo.includes(`version = "${version}"`)) throw new Error("Helper and desktop versions differ")
  await mkdir(resolve(root, target), { recursive: true })
  for (const [name, source] of [
    [helperName, resolve(repo, "src-tauri/host/target", triple, `release/${helperName}`)],
    ...resource.files.map(file => [file.path, resolve(herdrRoot, target, file.path)])
  ]) {
    const destination = resolve(root, target, name)
    await mkdir(dirname(destination), { recursive: true })
    await copyFile(source, `${destination}.tmp`)
    await chmod(`${destination}.tmp`, 0o755)
    await rename(`${destination}.tmp`, destination)
  }
  const helperHash = createHash("sha256").update(await readFile(resolve(root, target, helperName))).digest("hex")
  const artifact: HostArtifact = {
    protocol: 1, version, target,
    helper: { path: `${target}/${helperName}`, sha256: helperHash },
    herdr: { path: `${target}/${herdrName}`, sha256: resource.files.find(file => file.path === herdrName)!.sha256, version: HERDR_RESOURCE_VERSION.baseVersion, protocol: HERDR_RESOURCE_VERSION.protocol },
    files: resource.files.filter(file => file.path !== herdrName).map(file => ({ path: `${target}/${file.path}`, sha256: file.sha256 }))
  }
  validateHostArtifact(artifact)
  await copyFile(resolve(herdrRoot, "LICENSE-HERDR.txt"), resolve(root, "LICENSE-HERDR.txt"))
  await writeFile(resolve(root, `${target}.json.tmp`), JSON.stringify(artifact, null, 2) + "\n")
  await rename(resolve(root, `${target}.json.tmp`), resolve(root, `${target}.json`))
  return artifact
}

if (import.meta.main) {
  const platform = process.platform === "darwin" ? "macos" : process.platform === "win32" ? "windows" : process.platform
  const arch = process.arch === "arm64" ? "aarch64" : process.arch === "x64" ? "x86_64" : process.arch
  const target = process.argv[2] ?? `${platform}-${arch}`
  if (!Object.hasOwn(HOST_TARGETS, target)) throw new Error("Build helper artifacts on a supported target runner.")
  if (process.argv[3] && process.argv[3] !== "--zigbuild") throw new Error("Expected --zigbuild for cross compilation")
  console.log(JSON.stringify(await prepareHostResources(target as keyof typeof HOST_TARGETS, process.argv[3] ? "zigbuild" : "build")))
}

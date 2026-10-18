import { chmod, mkdir, readdir, readFile, rename, rm, rmdir, writeFile } from "node:fs/promises"
import { dirname, resolve } from "node:path"
import { fileURLToPath } from "node:url"
import { COMPANION_WEB_STAGING, verifyCompanionBundle } from "./verify-companion-bundle"

const repo = resolve(dirname(fileURLToPath(import.meta.url)), "..")

async function hostTriple(): Promise<string> {
  const rustc = Bun.spawn(["rustc", "-vV"], { stdout: "pipe", stderr: "inherit" })
  const output = await new Response(rustc.stdout).text()
  if (await rustc.exited !== 0) throw new Error("rustc -vV failed")
  const triple = /^host: (\S+)$/m.exec(output)?.[1]
  if (!triple) throw new Error("rustc -vV printed no host triple")
  return triple
}

/**
 * Tauri reruns the app crate's build script when an external binary or
 * resource changes, so an identical file is left alone: rewriting it would
 * recompile the app at every `tauri dev`.
 */
async function replace(destination: string, content: Buffer, mode?: number): Promise<boolean> {
  const current = await readFile(destination).catch(() => null)
  if (current?.equals(content)) return false
  await mkdir(dirname(destination), { recursive: true })
  await writeFile(`${destination}.tmp`, content)
  if (mode !== undefined) await chmod(`${destination}.tmp`, mode)
  await rename(`${destination}.tmp`, destination)
  return true
}

/** Mirrors `from` into `to` file by file, leaving identical files untouched. */
async function sync(from: string, to: string, files: string[]): Promise<boolean> {
  let changed = false
  for (const name of files) {
    if (await replace(resolve(to, name), await readFile(resolve(from, name)))) changed = true
  }
  const keep = new Set(files)
  const prune = async (dir: string, prefix: string): Promise<void> => {
    for (const entry of await readdir(dir, { withFileTypes: true }).catch(() => [])) {
      const name = `${prefix}${entry.name}`
      if (entry.isDirectory()) {
        await prune(resolve(dir, entry.name), `${name}/`)
        await rmdir(resolve(dir, entry.name)).catch(() => undefined)
      } else if (!keep.has(name)) {
        await rm(resolve(dir, entry.name))
        changed = true
      }
    }
  }
  await prune(to, "")
  return changed
}

/** Builds the Companion PWA into staging and checks it before the web root sees it. */
async function buildCompanionWeb(): Promise<string[]> {
  const build = Bun.spawn([process.execPath, "x", "vite", "build", "--config", "vite.companion.config.ts", "--logLevel", "warn"], { cwd: repo, stdout: "inherit", stderr: "inherit" })
  if (await build.exited !== 0) throw new Error("Companion PWA build failed")
  return (await verifyCompanionBundle(COMPANION_WEB_STAGING)).files
}

/**
 * Builds the gateway for the bundle's own target as the `externalBin` Tauri
 * expects (`binaries/yuzora-companion-<triple>`), and fills the web root with
 * the Companion PWA.
 * Its own target dir keeps it apart from the app crate's build.
 */
export async function prepareCompanion(triple: string, release: boolean): Promise<{ binary: string; webRoot: string; changed: boolean }> {
  const { version } = JSON.parse(await readFile(resolve(repo, "src-tauri/tauri.conf.json"), "utf8")) as { version: string }
  const cargo = await readFile(resolve(repo, "src-tauri/companion/Cargo.toml"), "utf8")
  if (!cargo.includes(`version = "${version}"`)) throw new Error("Companion and desktop versions differ")
  const targetDir = resolve(repo, "src-tauri/target/companion")
  const build = Bun.spawn([
    "cargo", "build", "--locked", ...(release ? ["--release"] : []),
    "--target", triple, "--target-dir", targetDir,
    "--manifest-path", resolve(repo, "src-tauri/companion/Cargo.toml"),
  ], { cwd: repo, stdout: "inherit", stderr: "inherit" })
  if (await build.exited !== 0) throw new Error(`Companion build failed for ${triple}`)
  const suffix = triple.includes("windows") ? ".exe" : ""
  const binary = resolve(repo, "src-tauri/binaries", `yuzora-companion-${triple}${suffix}`)
  const built = resolve(targetDir, triple, release ? "release" : "debug", `yuzora-companion${suffix}`)
  const binaryChanged = await replace(binary, await readFile(built), 0o755)
  const webRoot = resolve(repo, "src-tauri/resources/companion-web")
  const webChanged = await sync(COMPANION_WEB_STAGING, webRoot, await buildCompanionWeb())
  return { binary, webRoot, changed: binaryChanged || webChanged }
}

if (import.meta.main) {
  // Tauri sets these for beforeDevCommand / beforeBuildCommand.
  const triple = process.env.TAURI_ENV_TARGET_TRIPLE || await hostTriple()
  const release = process.env.TAURI_ENV_DEBUG !== "true"
  console.log(JSON.stringify(await prepareCompanion(triple, release)))
}

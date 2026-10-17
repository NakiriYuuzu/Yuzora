import { chmod, mkdir, readFile, rename, writeFile } from "node:fs/promises"
import { dirname, resolve } from "node:path"
import { fileURLToPath } from "node:url"

const repo = resolve(dirname(fileURLToPath(import.meta.url)), "..")

/** Served at `/` until the Companion PWA build (plan Phase 5) fills the web root. */
export const PLACEHOLDER_PAGE = `<!doctype html>
<html lang="en">
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>Yuzora Companion</title>
<p>The Yuzora Companion app is not included in this build yet.</p>
</html>
`

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

/**
 * Builds the gateway for the bundle's own target as the `externalBin` Tauri
 * expects (`binaries/yuzora-companion-<triple>`), and fills the web root.
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
  const webChanged = await replace(resolve(webRoot, "index.html"), Buffer.from(PLACEHOLDER_PAGE))
  return { binary, webRoot, changed: binaryChanged || webChanged }
}

if (import.meta.main) {
  // Tauri sets these for beforeDevCommand / beforeBuildCommand.
  const triple = process.env.TAURI_ENV_TARGET_TRIPLE || await hostTriple()
  const release = process.env.TAURI_ENV_DEBUG !== "true"
  console.log(JSON.stringify(await prepareCompanion(triple, release)))
}

import { spawnSync } from "node:child_process"
import { chmod, mkdir, readdir, readFile, rename, rm, rmdir, writeFile } from "node:fs/promises"
import { dirname, isAbsolute, relative, resolve, sep } from "node:path"
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
export async function replace(destination: string, content: Buffer, mode?: number): Promise<boolean> {
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

/** Rust target triple to the Go target of the Companion network sidecar. */
const GO_TARGETS = new Map([
  ["aarch64-apple-darwin", { goos: "darwin", goarch: "arm64" }],
  ["x86_64-apple-darwin", { goos: "darwin", goarch: "amd64" }],
  ["x86_64-pc-windows-msvc", { goos: "windows", goarch: "amd64" }],
  ["aarch64-pc-windows-msvc", { goos: "windows", goarch: "arm64" }],
  ["x86_64-unknown-linux-gnu", { goos: "linux", goarch: "amd64" }],
  ["aarch64-unknown-linux-gnu", { goos: "linux", goarch: "arm64" }],
])

export function goTarget(triple: string): { goos: string; goarch: string } {
  const target = GO_TARGETS.get(triple)
  if (!target) throw new Error(`The Companion network sidecar has no Go target for ${triple}`)
  return target
}

/** Runs `go` in the sidecar module and returns its stdout. */
async function runGo(args: string[], env: Record<string, string>): Promise<string> {
  let go
  try {
    go = Bun.spawn(["go", ...args], { cwd: resolve(repo, "src-tauri/companion-net"), env: { ...process.env, ...env }, stdout: "pipe", stderr: "inherit" })
  } catch (error) {
    if ((error as NodeJS.ErrnoException).code === "ENOENT") {
      throw new Error("Go 1.27 or newer is required to build the Companion network sidecar: install it from https://go.dev/dl/", { cause: error })
    }
    throw error
  }
  const output = await new Response(go.stdout).text()
  if (await go.exited !== 0) throw new Error(`go ${args[0]} failed for the Companion network sidecar`)
  return output
}

/** The tsnet web client is not used: omitting it also drops its prebuilt UI and the third-party font inside. `go list` must see the same tags as `go build`. */
const NET_BUILD_TAGS = "ts_omit_webclient"

/** One line per linked package, tab-separated because directories may contain spaces. */
const MODULE_LIST_FORMAT = "{{$pkg := .Dir}}{{with .Module}}{{.Path}}\t{{.Version}}\t{{.Dir}}\t{{$pkg}}{{end}}"

export type LinkedModule = { path: string; version: string; dir: string; packageDirs: string[] }

/** Parses `go list -deps -f MODULE_LIST_FORMAT`: the linked packages of each versioned module, leaving out the main module. */
export function parseModules(output: string): LinkedModule[] {
  const modules = new Map<string, LinkedModule>()
  for (const line of output.split(/\r?\n/)) {
    const [path, version, dir, packageDir] = line.split("\t")
    if (!path || !version || !dir || !packageDir) continue
    const module = modules.get(path) ?? { path, version, dir, packageDirs: [] }
    if (!module.packageDirs.includes(packageDir)) module.packageDirs.push(packageDir)
    modules.set(path, module)
  }
  return [...modules.values()].sort((a, b) => (a.path < b.path ? -1 : a.path > b.path ? 1 : 0))
}

const LICENSE_FILE = /^(LICENSE|COPYING|NOTICE)/

/**
 * The license files that apply to the linked code of one module: the ones at
 * its root and in every directory down to each linked package, so a license a
 * nested package carries (zstd/internal/xxhash) is not lost. Shallowest first;
 * a text found at several levels is listed once.
 */
export async function collectLicenseFiles(moduleDir: string, packageDirs: string[]): Promise<{ name: string; text: string }[]> {
  const dirs = new Map<string, string[]>([[moduleDir, []]])
  for (const packageDir of packageDirs) {
    const within = relative(moduleDir, packageDir)
    if (within === ".." || within.startsWith(`..${sep}`) || isAbsolute(within)) continue
    const segments = within.split(sep).filter(Boolean)
    for (let depth = 1; depth <= segments.length; depth++) dirs.set(resolve(moduleDir, ...segments.slice(0, depth)), segments.slice(0, depth))
  }
  const files: { name: string; text: string }[] = []
  const seen = new Set<string>()
  for (const [dir, segments] of [...dirs].sort((a, b) => a[1].length - b[1].length || (a[0] < b[0] ? -1 : 1))) {
    const names = (await readdir(dir, { withFileTypes: true })).filter((entry) => entry.isFile() && LICENSE_FILE.test(entry.name)).map((entry) => entry.name).sort()
    for (const name of names) {
      const text = await readFile(resolve(dir, name), "utf8")
      const key = text.replace(/\r\n/g, "\n").trim()
      if (seen.has(key)) continue
      seen.add(key)
      files.push({ name: [...segments, name].join("/"), text })
    }
  }
  return files
}

/** Go's license first, then every module's license files, in module path order. */
export function renderNotices(goLicense: string, modules: { path: string; version: string; files: { name: string; text: string }[] }[]): string {
  const section = (title: string, text: string) => `=== ${title} ===\n\n${text.replace(/\r\n/g, "\n").trimEnd()}\n`
  return [
    "Third-party notices for yuzora-companion-net, which includes the Go standard library and the modules below.\n",
    section("Go standard library: LICENSE", goLicense),
    ...modules.flatMap(({ path, version, files }) => files.map(({ name, text }) => section(`${path} ${version}: ${name}`, text))),
  ].join("\n")
}

/** A version no release has: the sidecar rejects it and names the version it was built for. */
const NET_VERSION_PROBE = "0.0.0-version-probe"

/** Throws unless the sidecar's answer to an init with the wrong version names `appVersion` as its own. */
export function checkNetVersionReply(stdout: string, appVersion: string): void {
  const line = stdout.split(/\r?\n/).find((text) => text.trim() !== "") ?? ""
  let frame: { type?: unknown; code?: unknown; message?: unknown } | null = null
  try {
    frame = JSON.parse(line)
  } catch {
    // Reported below with the raw output.
  }
  if (frame?.type !== "error" || frame.code !== "net-version-mismatch" || typeof frame.message !== "string") {
    throw new Error(`yuzora-companion-net did not answer the version probe with net-version-mismatch: ${stdout.trim().slice(0, 200)}`)
  }
  const built = /yuzora-companion-net is (\S+)$/.exec(frame.message)?.[1]
  if (built !== appVersion) {
    throw new Error(`yuzora-companion-net reports version ${built ?? frame.message} but the desktop is ${appVersion}: the -X main.version link did not take effect`)
  }
}

/** Whether this machine can execute a Go target (cross-built sidecars are not run). */
export function canRunGoTarget(target: { goos: string; goarch: string }, platform: string = process.platform, arch: string = process.arch): boolean {
  const goos = new Map([["darwin", "darwin"], ["win32", "windows"], ["linux", "linux"]]).get(platform)
  const goarch = new Map([["arm64", "arm64"], ["x64", "amd64"]]).get(arch)
  return target.goos === goos && target.goarch === goarch
}

/**
 * Runs the built sidecar with an init for a version nobody has. Go ignores
 * `-X` for a variable that does not exist, so a typo in the link flag would
 * ship a sidecar that refuses to start with every real desktop version.
 */
export function verifyNetVersion(binary: string, appVersion: string): void {
  const input = `${JSON.stringify({ v: 1, type: "init", stateDir: dirname(binary), hostname: "yuzora", version: NET_VERSION_PROBE })}\n`
  const probe = spawnSync(binary, [], { input, encoding: "utf8", timeout: 10_000 })
  if (probe.error) throw new Error(`could not run ${binary} to check its version`, { cause: probe.error })
  checkNetVersionReply(probe.stdout, appVersion)
}

/**
 * Builds the tailnet sidecar for the bundle's own target as the second
 * `externalBin` (`binaries/yuzora-companion-net-<triple>`) and writes the
 * license notices of everything linked into it. The build is reproducible, so
 * an unchanged sidecar leaves the copy alone like the gateway does. When this
 * machine can run the result, its version is checked before it is installed.
 */
export async function prepareCompanionNet(triple: string, appVersion: string): Promise<{ binary: string; notices: string; changed: boolean }> {
  const target = goTarget(triple)
  // Pure Go: no C toolchain needed to cross-compile, and the shipped binary is static.
  const env = { CGO_ENABLED: "0", GOOS: target.goos, GOARCH: target.goarch }
  const suffix = triple.includes("windows") ? ".exe" : ""
  const built = resolve(repo, "src-tauri/target/companion-net", triple, `yuzora-companion-net${suffix}`)
  await runGo(["build", "-trimpath", "-buildvcs=false", `-tags=${NET_BUILD_TAGS}`, "-ldflags", `-s -w -X main.version=${appVersion}`, "-o", built, "."], env)
  if (canRunGoTarget(target)) verifyNetVersion(built, appVersion)
  const binary = resolve(repo, "src-tauri/binaries", `yuzora-companion-net-${triple}${suffix}`)
  const binaryChanged = await replace(binary, await readFile(built), 0o755)

  const modules = []
  for (const { path, version, dir, packageDirs } of parseModules(await runGo(["list", "-deps", `-tags=${NET_BUILD_TAGS}`, "-f", MODULE_LIST_FORMAT, "."], env))) {
    const files = await collectLicenseFiles(dir, packageDirs)
    if (files.length === 0) throw new Error(`${path} ${version} ships no license file for the third-party notices`)
    modules.push({ path, version, files })
  }
  const goRoot = (await runGo(["env", "GOROOT"], env)).trim()
  const notices = resolve(repo, "src-tauri/resources/companion-net/THIRD_PARTY_NOTICES.txt")
  const noticesChanged = await replace(notices, Buffer.from(renderNotices(await readFile(resolve(goRoot, "LICENSE"), "utf8"), modules)))
  return { binary, notices, changed: binaryChanged || noticesChanged }
}

/** Builds the Companion PWA into staging and checks it before the web root sees it. */
async function buildCompanionWeb(): Promise<string[]> {
  const build = Bun.spawn([process.execPath, "x", "vite", "build", "--config", "vite.companion.config.ts", "--logLevel", "warn"], { cwd: repo, stdout: "inherit", stderr: "inherit" })
  if (await build.exited !== 0) throw new Error("Companion PWA build failed")
  return (await verifyCompanionBundle(COMPANION_WEB_STAGING)).files
}

/**
 * Builds the gateway for the bundle's own target as the `externalBin` Tauri
 * expects (`binaries/yuzora-companion-<triple>`), then the tailnet sidecar, and
 * fills the web root with the Companion PWA.
 * Its own target dir keeps it apart from the app crate's build.
 */
export async function prepareCompanion(triple: string, release: boolean): Promise<{ binary: string; netBinary: string; notices: string; webRoot: string; changed: boolean }> {
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
  const net = await prepareCompanionNet(triple, version)
  const webRoot = resolve(repo, "src-tauri/resources/companion-web")
  const webChanged = await sync(COMPANION_WEB_STAGING, webRoot, await buildCompanionWeb())
  return { binary, netBinary: net.binary, notices: net.notices, webRoot, changed: binaryChanged || net.changed || webChanged }
}

if (import.meta.main) {
  // Tauri sets these for beforeDevCommand / beforeBuildCommand.
  const triple = process.env.TAURI_ENV_TARGET_TRIPLE || await hostTriple()
  const release = process.env.TAURI_ENV_DEBUG !== "true"
  console.log(JSON.stringify(await prepareCompanion(triple, release)))
}

import { readdir, readFile } from "node:fs/promises"
import { dirname, resolve } from "node:path"
import { fileURLToPath } from "node:url"
import { gzipSync } from "node:zlib"

const repo = resolve(dirname(fileURLToPath(import.meta.url)), "..")
export const COMPANION_WEB_STAGING = resolve(repo, "src-tauri/target/companion/web")
export const GZIP_BUDGET = 2.5 * 1024 * 1024
const ROOT_FILES = new Set(["index.html", "manifest.webmanifest", "sw.js"])
const DESKTOP_ONLY = ["__TAURI", "@tauri-apps", "ipc.localhost", "tauri://"]

/** Mirrors the gateway's `valid_static_segment` (src-tauri/companion/src/http.rs). */
export function validStaticSegment(segment: string): boolean {
  if (!/^[A-Za-z0-9_-][A-Za-z0-9._-]*$/.test(segment) || segment.endsWith(".")) return false
  const stem = segment.split(".")[0].toUpperCase()
  return !["CON", "PRN", "AUX", "NUL"].includes(stem) && !/^(COM|LPT)[1-9]$/.test(stem)
}

async function walk(dir: string, prefix = ""): Promise<string[]> {
  const files: string[] = []
  for (const entry of await readdir(dir, { withFileTypes: true })) {
    const path = `${prefix}${entry.name}`
    if (entry.isDirectory()) files.push(...await walk(resolve(dir, entry.name), `${path}/`))
    else files.push(path)
  }
  return files.sort()
}

/** A shell reference the gateway serves: a root-relative path, never another origin. */
function local(url: string): boolean {
  return url.startsWith("/") && !url.startsWith("//")
}

/**
 * Checks a built web root against what the gateway serves and its CSP: only
 * the routed paths, no inline script, nothing from another origin, no desktop
 * APIs, a service worker that precaches exactly the shell, and the size budget.
 */
export async function verifyCompanionBundle(dir: string): Promise<{ files: string[]; gzipBytes: number }> {
  const files = await walk(dir)
  const problems: string[] = []
  const text = async (name: string) => readFile(resolve(dir, name), "utf8")
  let gzipBytes = 0
  for (const name of files) {
    const [top, ...rest] = name.split("/")
    if (!name.split("/").every(validStaticSegment)) problems.push(`${name}: not a servable path`)
    else if (rest.length === 0 ? !ROOT_FILES.has(top) : top !== "assets" && top !== "icons") problems.push(`${name}: outside the served routes`)
    const bytes = await readFile(resolve(dir, name))
    gzipBytes += gzipSync(bytes).length
    if (/\.(js|css|html)$/.test(name)) {
      const content = bytes.toString("utf8")
      for (const marker of DESKTOP_ONLY) if (content.includes(marker)) problems.push(`${name}: contains desktop-only ${marker}`)
      if (content.includes("sourceMappingURL")) problems.push(`${name}: references a source map`)
    }
    if (name.endsWith(".map") || name.endsWith(".wasm")) problems.push(`${name}: not allowed in the shell`)
  }
  if (gzipBytes > GZIP_BUDGET) problems.push(`gzip total ${gzipBytes} bytes exceeds ${GZIP_BUDGET}`)
  for (const required of ROOT_FILES) if (!files.includes(required)) problems.push(`${required}: missing`)
  const served = (url: string) => files.includes(url === "/" ? "index.html" : url.slice(1))

  if (files.includes("index.html")) {
    const html = await text("index.html")
    for (const tag of html.match(/<script\b[^>]*>[\s\S]*?<\/script>/gi) ?? []) {
      if (!/\bsrc=/.test(tag) || !/>\s*<\/script>$/.test(tag)) problems.push("index.html: inline script")
    }
    if (/\son[a-z]+\s*=/i.test(html)) problems.push("index.html: inline event handler")
    if (/javascript:/i.test(html)) problems.push("index.html: javascript: URL")
    for (const [, url] of html.matchAll(/\b(?:src|href)="([^"]*)"/g)) {
      if (!local(url)) problems.push(`index.html: ${url} is not a local path`)
      else if (!served(url)) problems.push(`index.html: ${url} is not in the bundle`)
    }
  }
  for (const name of files.filter((file) => file.endsWith(".css"))) {
    const css = await text(name)
    for (const [, raw] of css.matchAll(/url\(\s*([^)]*?)\s*\)/g)) {
      const url = raw.replace(/^["']|["']$/g, "")
      if (!local(url)) problems.push(`${name}: ${url.slice(0, 40)} is not a local path`)
      else if (!served(url)) problems.push(`${name}: ${url} is not in the bundle`)
    }
    if (/@import\s/.test(css)) problems.push(`${name}: runtime @import`)
  }
  if (files.includes("manifest.webmanifest")) {
    const manifest = JSON.parse(await text("manifest.webmanifest")) as { start_url?: string; scope?: string; display?: string; icons?: { src: string; sizes: string }[] }
    if (manifest.start_url !== "/" || manifest.scope !== "/" || manifest.display !== "standalone") problems.push("manifest.webmanifest: start_url/scope must be / and display standalone")
    const icons = manifest.icons ?? []
    for (const size of ["192x192", "512x512"]) if (!icons.some((icon) => icon.sizes === size)) problems.push(`manifest.webmanifest: no ${size} icon`)
    for (const icon of icons) if (!local(icon.src) || !served(icon.src)) problems.push(`manifest.webmanifest: icon ${icon.src} is not in the bundle`)
  }
  if (files.includes("sw.js")) {
    const sw = await text("sw.js")
    if (sw.includes("__YZC_")) problems.push("sw.js: unfilled placeholder")
    const list = /const PRECACHE = (\[[^\]]*\])/.exec(sw)?.[1]
    const precache = list ? JSON.parse(list) as string[] : []
    if (!list) problems.push("sw.js: no precache list")
    for (const url of precache) if (!served(url)) problems.push(`sw.js: precaches ${url}, which is not in the bundle`)
    const shell = ["/", ...files.filter((file) => !ROOT_FILES.has(file)).map((file) => `/${file}`), "/manifest.webmanifest"]
    for (const url of shell) if (!precache.includes(url)) problems.push(`sw.js: does not precache ${url}`)
    if (precache.some((url) => url.startsWith("/api/") || url === "/ws" || url === "/sw.js")) problems.push("sw.js: precaches an API, socket or itself")
  }
  if (problems.length) throw new Error(`Companion bundle check failed:\n${problems.map((p) => `  - ${p}`).join("\n")}`)
  return { files, gzipBytes }
}

if (import.meta.main) {
  const dir = process.argv[2] ? resolve(process.argv[2]) : COMPANION_WEB_STAGING
  const { files, gzipBytes } = await verifyCompanionBundle(dir)
  console.log(`Companion bundle OK: ${files.length} files, ${(gzipBytes / 1024).toFixed(1)} KiB gzip`)
}

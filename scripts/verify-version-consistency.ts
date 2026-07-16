import { readFileSync } from "node:fs"
import { resolve } from "node:path"

const root = process.cwd()
const readJson = (path: string) => JSON.parse(readFileSync(resolve(root, path), "utf8"))

const tauriVersion = readJson("src-tauri/tauri.conf.json").version as unknown
const packageVersion = readJson("package.json").version as unknown
const cargoToml = readFileSync(resolve(root, "src-tauri/Cargo.toml"), "utf8")
const cargoVersion = cargoToml.match(/^\[package\][\s\S]*?^version\s*=\s*"([^"]+)"/m)?.[1]

if (typeof tauriVersion !== "string" || !tauriVersion) {
  throw new Error("tauri.conf.json must define a non-empty product version")
}

const mismatches: string[] = []
if (packageVersion !== tauriVersion) {
  mismatches.push(`package.json version ${String(packageVersion)} != ${tauriVersion}`)
}
if (cargoVersion !== tauriVersion) {
  mismatches.push(`Cargo.toml version ${String(cargoVersion)} != ${tauriVersion}`)
}

const tag = process.env.GITHUB_REF_NAME
if (tag && tag !== `v${tauriVersion}`) {
  mismatches.push(`tag ${tag} != v${tauriVersion}`)
}

if (mismatches.length > 0) {
  for (const mismatch of mismatches) console.error(`::error::${mismatch}`)
  process.exit(1)
}

console.log(`Version consistency verified: v${tauriVersion}`)

import { readFileSync } from "node:fs"
import { resolve } from "node:path"

import { assertReleaseVersion, versionFromTag } from "./release-version"

export function verifyVersionConsistency(root = process.cwd(), tag = process.env.GITHUB_REF_NAME) {
  const readJson = (path: string) => JSON.parse(readFileSync(resolve(root, path), "utf8"))
  const tauriVersion = readJson("src-tauri/tauri.conf.json").version as unknown
  const packageVersion = readJson("package.json").version as unknown
  const cargoToml = readFileSync(resolve(root, "src-tauri/Cargo.toml"), "utf8")
  const cargoLock = readFileSync(resolve(root, "src-tauri/Cargo.lock"), "utf8")
  const cargoVersion = cargoToml.match(/^\[package\][\s\S]*?^version\s*=\s*"([^"]+)"/m)?.[1]
  const cargoLockVersion = cargoLock.match(/\[\[package\]\]\nname = "yuzora"\nversion = "([^"]+)"/)?.[1]

  if (typeof tauriVersion !== "string" || !tauriVersion) {
    throw new Error("tauri.conf.json must define a non-empty product version")
  }

  assertReleaseVersion(tauriVersion)
  const mismatches: string[] = []
  if (packageVersion !== tauriVersion) {
    mismatches.push(`package.json version ${String(packageVersion)} != ${tauriVersion}`)
  }
  if (cargoVersion !== tauriVersion) {
    mismatches.push(`Cargo.toml version ${String(cargoVersion)} != ${tauriVersion}`)
  }
  if (cargoLockVersion !== tauriVersion) {
    mismatches.push(`Cargo.lock root package version ${String(cargoLockVersion)} != ${tauriVersion}`)
  }
  if (tag && versionFromTag(tag) !== tauriVersion) {
    mismatches.push(`tag ${tag} != v${tauriVersion}`)
  }

  if (mismatches.length > 0) {
    throw new Error(mismatches.join("\n"))
  }

  return `Version consistency verified: v${tauriVersion}`
}

if (import.meta.main) {
  try {
    console.log(verifyVersionConsistency())
  } catch (error) {
    console.error(`::error::${error instanceof Error ? error.message : String(error)}`)
    process.exit(1)
  }
}

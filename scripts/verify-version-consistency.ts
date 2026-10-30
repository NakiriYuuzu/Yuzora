import { readFileSync } from "node:fs"
import { resolve } from "node:path"

import { assertReleaseVersion, versionFromTag } from "./release-version"

export function verifyVersionConsistency(root = process.cwd(), tag = process.env.GITHUB_REF_NAME) {
  const readJson = (path: string) => JSON.parse(readFileSync(resolve(root, path), "utf8"))
  const tauriVersion = readJson("src-tauri/tauri.conf.json").version as unknown
  const packageVersion = readJson("package.json").version as unknown
  const cargoToml = readFileSync(resolve(root, "src-tauri/Cargo.toml"), "utf8")
  const hostCargoToml = readFileSync(resolve(root, "src-tauri/host/Cargo.toml"), "utf8")
  const companionCargoToml = readFileSync(resolve(root, "src-tauri/companion/Cargo.toml"), "utf8")
  // One workspace lock holds every member's entry.
  const cargoLock = readFileSync(resolve(root, "Cargo.lock"), "utf8")
  const lockVersion = (name: string) =>
    cargoLock.match(new RegExp(`\\[\\[package\\]\\]\\r?\\nname = "${name}"\\r?\\nversion = "([^"]+)"`))?.[1]
  const cargoVersion = cargoToml.match(/^\[package\][\s\S]*?^version\s*=\s*"([^"]+)"/m)?.[1]
  const cargoLockVersion = lockVersion("yuzora")
  const hostCargoVersion = hostCargoToml.match(/^\[package\][\s\S]*?^version\s*=\s*"([^"]+)"/m)?.[1]
  const hostCargoLockVersion = lockVersion("yuzora-host")
  const companionCargoVersion = companionCargoToml.match(/^\[package\][\s\S]*?^version\s*=\s*"([^"]+)"/m)?.[1]
  const companionCargoLockVersion = lockVersion("yuzora-companion")

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
    mismatches.push(`Cargo.lock app package version ${String(cargoLockVersion)} != ${tauriVersion}`)
  }
  if (hostCargoVersion !== tauriVersion) {
    mismatches.push(`host Cargo.toml version ${String(hostCargoVersion)} != ${tauriVersion}`)
  }
  if (hostCargoLockVersion !== tauriVersion) {
    mismatches.push(`Cargo.lock host package version ${String(hostCargoLockVersion)} != ${tauriVersion}`)
  }
  if (companionCargoVersion !== tauriVersion) {
    mismatches.push(`companion Cargo.toml version ${String(companionCargoVersion)} != ${tauriVersion}`)
  }
  if (companionCargoLockVersion !== tauriVersion) {
    mismatches.push(`Cargo.lock companion package version ${String(companionCargoLockVersion)} != ${tauriVersion}`)
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

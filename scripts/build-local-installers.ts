import { spawnSync } from "node:child_process"
import { readFileSync } from "node:fs"

import { releaseMsiBuildConfig } from "./release-msi-build-config"

export function buildLocalInstallerArgs(version: string, extraArgs: string[] = []): string[] {
  return [
    "tauri",
    "build",
    "--ci",
    "--no-sign",
    "--config",
    JSON.stringify(releaseMsiBuildConfig(version, true)),
    ...extraArgs,
  ]
}

if (import.meta.main) {
  const { version } = JSON.parse(readFileSync("package.json", "utf8")) as { version: string }
  const result = spawnSync("bun", buildLocalInstallerArgs(version, process.argv.slice(2)), {
    stdio: "inherit",
  })
  if (result.error) {
    console.error(result.error.message)
    process.exit(1)
  }
  process.exit(result.status ?? 1)
}

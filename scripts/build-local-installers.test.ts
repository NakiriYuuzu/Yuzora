import { describe, expect, it } from "vitest"

import { buildLocalInstallerArgs } from "./build-local-installers"

describe("local installer build", () => {
  it("disables updater artifacts and preserves macOS bundle signing", () => {
    const args = buildLocalInstallerArgs("0.0.9-beta.3")
    expect(args.slice(0, process.platform === "darwin" ? 3 : 4)).toEqual(
      process.platform === "darwin"
        ? ["tauri", "build", "--ci"]
        : ["tauri", "build", "--ci", "--no-sign"],
    )
    const configIndex = args.indexOf("--config")
    expect(configIndex).toBeGreaterThan(0)
    expect(JSON.parse(args[configIndex + 1]!)).toEqual({
      bundle: {
        createUpdaterArtifacts: false,
        windows: { wix: { version: "0.0.2307" } },
      },
      plugins: { updater: { endpoints: [] } },
    })
  })
})

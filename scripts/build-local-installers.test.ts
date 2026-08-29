import { describe, expect, it } from "vitest"

import { buildLocalInstallerArgs } from "./build-local-installers"

describe("local installer build", () => {
  it("disables updater artifacts and signing without changing the product version", () => {
    const args = buildLocalInstallerArgs("0.0.9-beta.2")
    expect(args.slice(0, 4)).toEqual(["tauri", "build", "--ci", "--no-sign"])
    expect(args[4]).toBe("--config")
    expect(JSON.parse(args[5])).toEqual({
      bundle: {
        createUpdaterArtifacts: false,
        windows: { wix: { version: "0.0.2306" } },
      },
      plugins: { updater: { endpoints: [] } },
    })
  })
})

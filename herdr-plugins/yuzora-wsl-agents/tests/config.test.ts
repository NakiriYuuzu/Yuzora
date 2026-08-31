import { describe, expect, it } from "vitest"
import { parsePluginConfig, resolveLaunchDistro, resolveTargetDistros } from "../lib/config"

describe("plugin config", () => {
  it("accepts the MVP schema and defaults", () => {
    expect(parsePluginConfig(undefined).enabledAgents).toEqual(["pi"])
    expect(
      parsePluginConfig({
        schemaVersion: 1,
        defaultDistro: "Ubuntu",
        distros: ["Ubuntu", "Debian"],
        enabledAgents: ["pi"],
        linuxCwdPolicy: "workspace"
      }).distros
    ).toEqual(["Ubuntu", "Debian"])
  })

  it("rejects unknown keys, traversal, and non-pi agents", () => {
    expect(() => parsePluginConfig({ extra: true })).toThrow(/unknown plugin config key/)
    expect(() => parsePluginConfig({ defaultDistro: "../x" })).toThrow(/unsafe/)
    expect(() => parsePluginConfig({ enabledAgents: ["claude"] })).toThrow(/exactly/)
    expect(() => parsePluginConfig({ schemaVersion: 2 })).toThrow(/schemaVersion/)
  })

  it("resolves configured distros against inventory and fails closed", () => {
    const config = parsePluginConfig({
      defaultDistro: "Ubuntu",
      distros: ["ubuntu"],
      enabledAgents: ["pi"]
    })
    expect(resolveTargetDistros(config, ["Ubuntu", "Debian"])).toEqual(["Ubuntu"])
    expect(() => resolveTargetDistros(config, ["Debian"])).toThrow(/not installed/)
  })

  it("opens panes on defaultDistro without requiring every adapter target", () => {
    const config = parsePluginConfig({
      defaultDistro: "Ubuntu",
      distros: ["Debian", "Ubuntu"],
      enabledAgents: ["pi"]
    })
    expect(resolveLaunchDistro(config, ["Ubuntu"])).toBe("Ubuntu")
    expect(resolveLaunchDistro(config, ["Debian", "Ubuntu"])).toBe("Ubuntu")
    expect(() => resolveTargetDistros(config, ["Ubuntu"])).toThrow(/Debian/)
    expect(resolveLaunchDistro(parsePluginConfig({ enabledAgents: ["pi"] }), ["Ubuntu"])).toBeNull()
  })
})

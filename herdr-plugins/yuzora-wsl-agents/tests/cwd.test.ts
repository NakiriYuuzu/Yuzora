import { describe, expect, it } from "vitest"
import { parseWslUnc, planLaunchCwd, stripVerbatimWindowsPrefix, wslLaunchArgs } from "../lib/cwd"

describe("cwd conversion", () => {
  it("parses wsl.localhost and wsl$ UNC including CJK", () => {
    expect(parseWslUnc("\\\\wsl.localhost\\Ubuntu\\home\\yuuzu\\專案")).toEqual({
      distro: "Ubuntu",
      linuxCwd: "/home/yuuzu/專案"
    })
    expect(parseWslUnc("\\\\wsl$\\Debian\\home\\yuuzu")).toEqual({
      distro: "Debian",
      linuxCwd: "/home/yuuzu"
    })
    expect(parseWslUnc("C:\\Users\\yuuzu\\project")).toBeNull()
  })

  it("strips verbatim prefixes and rejects distro mismatch", () => {
    expect(stripVerbatimWindowsPrefix("\\\\?\\C:\\Users\\yuuzu\\My Project")).toBe(
      "C:\\Users\\yuuzu\\My Project"
    )
    expect(stripVerbatimWindowsPrefix("\\\\?\\UNC\\wsl.localhost\\Ubuntu\\tmp")).toBe(
      "\\\\wsl.localhost\\Ubuntu\\tmp"
    )
    expect(() =>
      planLaunchCwd({
        workspacePath: "\\\\wsl.localhost\\Ubuntu\\home",
        configuredDistro: "Debian",
        linuxCwdPolicy: "workspace"
      })
    ).toThrow(/does not match/)
  })

  it("builds argv without interpolating user strings into a shell", () => {
    expect(
      wslLaunchArgs({
        distro: "Ubuntu",
        linuxCwd: "/home/yuuzu/My Project",
        kind: "pi"
      })
    ).toEqual(["--distribution", "Ubuntu", "--cd", "/home/yuuzu/My Project", "--", "pi"])
    expect(wslLaunchArgs({ distro: null, linuxCwd: null, kind: "shell" })).toEqual([])
    expect(
      planLaunchCwd({
        workspacePath: "C:\\Users\\yuuzu\\專案",
        configuredDistro: "Ubuntu",
        linuxCwdPolicy: "home"
      })
    ).toEqual({ kind: "home" })
  })
})

import { describe, expect, it } from "vitest"
import { REPORTER_CHILD_WSLENV, WIN32_TO_WSL_WSLENV } from "../lib/constants"
import {
  containsRawSocketEntry,
  denyHerdrSocketPathEntries,
  mergeReporterChildWslenv,
  mergeWin32ToWslEnv,
  splitWslenv
} from "../lib/wslenv"

describe("WSLENV deny-list", () => {
  it("removes HERDR_SOCKET_PATH regardless of flags or case", () => {
    const existing =
      "FOO/u:herdr_socket_path:BAR/p:HERDR_SOCKET_PATH/up:HERDR_SOCKET_PATH/w:baz"
    const merged = mergeWin32ToWslEnv(existing, WIN32_TO_WSL_WSLENV)
    expect(containsRawSocketEntry(merged)).toBe(false)
    expect(merged).toContain("YUZORA_HERDR_SOCKET_PATH/u")
    expect(merged).toContain("HERDR_BIN_PATH/up")
    expect(merged).toContain("FOO/u")
    expect(merged).toContain("BAR/p")
    expect(
      splitWslenv(merged).some((entry) => /^HERDR_SOCKET_PATH(?:\/|$)/i.test(entry))
    ).toBe(false)
  })

  it("preserves unrelated entries and last-wins on names", () => {
    const merged = mergeWin32ToWslEnv("FOO/u:FOO/p:HERDR_ENV/l", ["HERDR_ENV/u", "FOO/w"])
    expect(denyHerdrSocketPathEntries(splitWslenv(merged))).toEqual(splitWslenv(merged))
    expect(merged.split(":")).toContain("FOO/w")
    expect(merged.split(":")).toContain("HERDR_ENV/u")
    expect(merged.split(":")).not.toContain("FOO/u")
  })

  it("adds HERDR_SOCKET_PATH/w only for the Windows herdr.exe child", () => {
    const child = mergeReporterChildWslenv(
      "FOO/u:HERDR_SOCKET_PATH/up:YUZORA_HERDR_SOCKET_PATH/u",
      REPORTER_CHILD_WSLENV
    )
    expect(child.split(":")).toContain("HERDR_SOCKET_PATH/w")
    expect(child.split(":")).not.toContain("HERDR_SOCKET_PATH/up")
    expect(child.split(":")).toContain("YUZORA_HERDR_SOCKET_PATH/u")
  })
})

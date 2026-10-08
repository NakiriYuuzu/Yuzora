import { describe, expect, it } from "vitest"
import i18n from "@/lib/i18n"
import enMessages from "./i18n/locales/en/herdrErrors.json"
import zhMessages from "./i18n/locales/zh-TW/herdrErrors.json"
import { describeHerdrError, herdrErrorKind, isRetryableHerdrConnectError } from "./herdrErrors"

describe("herdrErrorKind", () => {
  it("treats remote helper admission limits as backpressure", () => {
    for (const message of ["host-request-limit", "host-call-wait-timeout", "host-stream-open-limit", "host-stream-open-wait-timeout", "stream-closed-or-busy"]) {
      expect(herdrErrorKind(new Error(message))).toBe("busy")
    }
  })
})

describe("isRetryableHerdrConnectError", () => {
  it("retries transient connector failures", () => {
    for (const message of ["host-stream-open-limit", "host-stream-open-wait-timeout", "too-many-host-streams", "stream-request-timeout", "Remote connector is closed", "Runtime connection changed; response discarded"]) {
      expect(isRetryableHerdrConnectError(new Error(message))).toBe(true)
    }
  })

  it("does not retry a runtime that refused the connector", () => {
    for (const message of ["permission denied", "unknown method: terminal.session", "herdr capabilities unknown"]) {
      expect(isRetryableHerdrConnectError(new Error(message))).toBe(false)
    }
  })
})

describe("describeHerdrError", () => {
  const t = i18n.t.bind(i18n) as unknown as Parameters<typeof describeHerdrError>[1]
  const en = enMessages as Record<string, unknown>
  const zh = zhMessages as Record<string, unknown>
  const codes = Object.keys(en).filter((key) => key !== "ui")
  // Codes the Rust side is known to emit; removing one from both locales must also fail.
  const REQUIRED = ["herdr-custom-path-not-executable", "herdr-custom-path-not-exe", "herdr-custom-path-required", "herdr-not-found-on-path", "herdr-path-binary-not-found", "herdr-binary-unavailable", "runtime-incompatible", "wsl-runtime-disabled-open-settings", "runtime-preferences-unwritable", "host-artifact-missing", "native-client-limit", "herdr-session-incompatible"]

  it("ships every code in both locales with identical keys", () => {
    expect(Object.keys(zh).sort()).toEqual(Object.keys(en).sort())
    for (const code of REQUIRED) expect(codes).toContain(code)
    for (const code of codes) {
      expect(typeof en[code], code).toBe("string")
      expect(typeof zh[code], code).toBe("string")
      expect((zh[code] as string).length, code).toBeGreaterThan(0)
    }
  })

  it.each(["en", "zh-TW"])("localizes every code, with and without detail (%s)", async (lang) => {
    await i18n.changeLanguage(lang)
    const table = lang === "en" ? en : zh
    for (const code of codes) {
      const bare = describeHerdrError(code, t)
      expect(bare).toMatchObject({ code, detail: null, message: table[code], raw: code })
      const withDetail = describeHerdrError(`${code}: C:\\x y\\herdr.exe\nline two`, t)
      expect(withDetail).toMatchObject({ code, detail: "C:\\x y\\herdr.exe\nline two", message: table[code] })
    }
  })

  it("falls back to the raw text for unknown codes and plain sentences", () => {
    expect(describeHerdrError("some-unknown-code: boom", t)).toEqual({ code: null, detail: null, message: "some-unknown-code: boom", raw: "some-unknown-code: boom" })
    expect(describeHerdrError(new Error("server changed during check"), t)).toMatchObject({ code: null, message: "server changed during check" })
  })

  it("localizes the herdr-path-binary-not-found code", () => {
    expect(describeHerdrError("herdr-path-binary-not-found", t).code).toBe("herdr-path-binary-not-found")
  })

  it("maps legacy English sentences exactly and keeps the raw text", () => {
    expect(describeHerdrError("Herdr was not found on PATH", t)).toMatchObject({ code: "herdr-not-found-on-path", raw: "Herdr was not found on PATH" })
    expect(describeHerdrError("herdr binary is unavailable for startup", t).code).toBe("herdr-binary-unavailable")
    expect(describeHerdrError("herdr config directory is not configured", t).code).toBe("herdr-config-dir-not-configured")
    expect(describeHerdrError("herdr binary override is not executable: C:\\gone\\herdr.exe", t)).toMatchObject({ code: "herdr-custom-path-not-executable", detail: "C:\\gone\\herdr.exe" })
    expect(describeHerdrError("Herdr was not found on PATH today", t).code).toBeNull()
  })

  it("folds the long runtime-incompatible sentence into detail", () => {
    const raw = "runtime-incompatible: client 0.8 / protocol 20 at /bin/herdr; schema protocol 22; missing methods [\"a\"]; affects Sessions."
    const described = describeHerdrError(raw, t)
    expect(described.code).toBe("runtime-incompatible")
    expect(described.message).not.toContain("protocol 20")
    expect(described.detail).toContain("protocol 20")
    expect(described.raw).toBe(raw)
  })
})

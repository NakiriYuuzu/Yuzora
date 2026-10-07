import { describe, expect, it } from "vitest"
import { herdrErrorKind, isRetryableHerdrConnectError } from "./herdrErrors"

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

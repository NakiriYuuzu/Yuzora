import { describe, expect, it } from "vitest"
import { herdrErrorKind, isHerdrSnapshotContractError, isRetryableHerdrConnectError } from "./herdrErrors"

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


describe("isHerdrSnapshotContractError", () => {
  it.each([
    "herdr server protocol incompatible",
    "selected herdr schema lacks session.snapshot",
    "selected herdr schema lacks session.snapshot/tab.create",
    "herdr snapshot unavailable",
    "herdr server identity changed during capability discovery"
  ])("recognizes the exact snapshot gate signal: %s", message => {
    expect(isHerdrSnapshotContractError(message)).toBe(true)
    expect(isHerdrSnapshotContractError(new Error(message))).toBe(true)
  })

  it.each([
    "selected herdr schema lacks tab.create",
    "herdr api schema failed: selected herdr schema lacks session.snapshot",
    "herdr api schema failed: process timed out",
    "read failed: herdr snapshot unavailable",
    "connect fixture.sock failed: connection refused",
    "write failed: broken pipe",
    "host-request-timeout",
    "host-request-limit",
    "host-call-wait-timeout",
    "snapshot temporarily unavailable",
    "unknown error: herdr server protocol incompatible"
  ])("does not promote a transient or unrelated error into a contract signal: %s", message => {
    expect(isHerdrSnapshotContractError(new Error(message))).toBe(false)
  })
})

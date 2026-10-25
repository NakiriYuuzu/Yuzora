import { describe, expect, it } from "vitest"
import contract from "../../../src-tauri/companion/tests/fixtures/pwa-contract.json"
import { decodeExact, fromBase64Url, toBase64Url } from "./base64url"
import { defaultLabel, labelProblem } from "./label"
import { deriveSas } from "./sas"

// The gateway checks the same fixture (tests/devices_policy.rs).
describe("PWA ↔ gateway contract", () => {
  it("accepts and refuses exactly the labels the gateway does", () => {
    for (const label of contract.labels.valid) expect(labelProblem(label), JSON.stringify(label)).toBeNull()
    for (const label of contract.labels.invalid) expect(labelProblem(label), JSON.stringify(label)).not.toBeNull()
    expect(labelProblem("")).toBe("empty")
    expect(labelProblem("a".repeat(81))).toBe("too-long")
    expect(labelProblem(" a")).toBe("edge-space")
    expect(labelProblem("a‍b")).toBe("hidden-character")
  })

  it("derives the gateway's SAS", async () => {
    for (const vector of contract.sas) {
      const bytes = (text: string) => decodeExact(text, 32)!
      expect(await deriveSas(bytes(vector.secret), bytes(vector.deviceNonce), bytes(vector.gatewayNonce))).toBe(vector.sas)
    }
  })

  it("offers default labels that pass the check", () => {
    for (const agent of ["Mozilla/5.0 (iPhone; CPU iPhone OS 18_0)", "Mozilla/5.0 (Linux; Android 15)", "Mozilla/5.0 (iPad)", "curl"]) {
      expect(labelProblem(defaultLabel(agent))).toBeNull()
    }
  })
})

describe("base64url", () => {
  it("round-trips without padding and checks exact sizes", () => {
    const bytes = new Uint8Array([0, 255, 62, 63, 1])
    expect(toBase64Url(bytes)).toBe("AP8-PwE")
    expect([...fromBase64Url("AP8-PwE")]).toEqual([...bytes])
    expect(decodeExact("AQEBAQEBAQEBAQEBAQEBAQ", 16)).not.toBeNull()
    expect(decodeExact("AQEBAQEBAQEBAQEBAQEBAQ=", 16)).toBeNull()
    expect(decodeExact("AQEBAQEBAQEBAQEBAQEBA", 16)).toBeNull()
    expect(decodeExact("AQEBAQEBAQEBAQEBAQEBA+", 16)).toBeNull()
    expect(() => fromBase64Url("a")).toThrow()
  })
})

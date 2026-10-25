import { describe, expect, it } from "vitest"
import { toBase64Url } from "./base64url"
import { creationOptions, credentialJSON, passkeyError, requestOptions } from "./webauthn"

const bytes = (...values: number[]) => new Uint8Array(values)

describe("WebAuthn option conversion", () => {
  it("decodes the gateway's flat creation options", () => {
    const options = creationOptions({
      rp: { id: "desktop.tail1234.ts.net", name: "desktop.tail1234.ts.net" },
      user: { name: "yuzora-companion", id: "AQID", displayName: "iPhone" },
      challenge: "BAUG",
      pubKeyCredParams: [{ type: "public-key", alg: -8 }],
      timeout: 120000,
      excludeCredentials: [{ type: "public-key", id: "Bwg", transports: ["usb"] }],
      authenticatorSelection: { residentKey: "required", requireResidentKey: true, userVerification: "required" },
      attestation: "none",
      extensions: {},
    })
    expect([...new Uint8Array(options.challenge as ArrayBuffer)]).toEqual([4, 5, 6])
    expect([...new Uint8Array(options.user.id as ArrayBuffer)]).toEqual([1, 2, 3])
    expect(options.user.name).toBe("yuzora-companion")
    expect([...new Uint8Array(options.excludeCredentials![0].id as ArrayBuffer)]).toEqual([7, 8])
    expect(options.authenticatorSelection?.userVerification).toBe("required")
    expect(options.attestation).toBe("none")
  })

  it("decodes request options with and without allowed credentials", () => {
    expect(requestOptions({ challenge: "BAUG", rpId: "h", userVerification: "required" }).allowCredentials).toBeUndefined()
    const options = requestOptions({ challenge: "BAUG", rpId: "h", allowCredentials: [{ type: "public-key", id: "Bwg" }] })
    expect([...new Uint8Array(options.allowCredentials![0].id as ArrayBuffer)]).toEqual([7, 8])
  })
})

describe("credentialJSON", () => {
  const base = { id: "Bwg", rawId: bytes(7, 8).buffer, type: "public-key", authenticatorAttachment: null, getClientExtensionResults: () => ({}) }

  it("encodes a registration by hand when toJSON is missing, without a null attachment", () => {
    const json = credentialJSON({
      ...base,
      response: {
        clientDataJSON: bytes(1).buffer,
        attestationObject: bytes(2).buffer,
        getAuthenticatorData: () => bytes(3).buffer,
        getPublicKey: () => bytes(4).buffer,
        getPublicKeyAlgorithm: () => -7,
        getTransports: () => ["internal"],
      },
    } as unknown as PublicKeyCredential)
    expect(json).toEqual({
      id: "Bwg", rawId: "Bwg", type: "public-key", clientExtensionResults: {},
      response: { clientDataJSON: "AQ", attestationObject: "Ag", authenticatorData: "Aw", publicKey: "BA", publicKeyAlgorithm: -7, transports: ["internal"] },
    })
  })

  it("encodes an assertion by hand", () => {
    const json = credentialJSON({
      ...base,
      authenticatorAttachment: "platform",
      response: { clientDataJSON: bytes(1).buffer, authenticatorData: bytes(2).buffer, signature: bytes(3).buffer, userHandle: bytes(4).buffer },
    } as unknown as PublicKeyCredential)
    expect(json.authenticatorAttachment).toBe("platform")
    expect(json.response).toEqual({ clientDataJSON: "AQ", authenticatorData: "Ag", signature: "Aw", userHandle: toBase64Url(bytes(4)) })
  })

  it("prefers the browser's toJSON", () => {
    const json = credentialJSON({ ...base, toJSON: () => ({ id: "x", authenticatorAttachment: null, response: {} }), response: {} } as unknown as PublicKeyCredential)
    expect(json).toEqual({ id: "x", response: {} })
  })
})

describe("passkeyError", () => {
  it("classifies ceremony failures", () => {
    expect(passkeyError(new DOMException("", "NotAllowedError"))).toBe("cancelled")
    expect(passkeyError(new DOMException("", "InvalidStateError"))).toBe("exists")
    expect(passkeyError(new DOMException("", "NotSupportedError"))).toBe("unsupported")
    expect(passkeyError(new DOMException("", "SecurityError"))).toBe("security")
    expect(passkeyError(new Error("boom"))).toBe("failed")
  })
})

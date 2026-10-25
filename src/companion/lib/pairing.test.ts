import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"
import contract from "../../../src-tauri/companion/tests/fixtures/pwa-contract.json"
import * as base64url from "./base64url"
import { claimPairing, pollPairing, SasMismatchError, takePairingLink } from "./pairing"

const ID = "AQEBAQEBAQEBAQEBAQEBAQ"
const vector = contract.sas[0]
const fetchMock = vi.fn<typeof fetch>()
const json = (body: unknown, status = 200) => new Response(JSON.stringify(body), { status, headers: { "content-type": "application/json" } })

beforeEach(() => {
  fetchMock.mockReset()
  vi.stubGlobal("fetch", fetchMock)
})
afterEach(() => {
  vi.useRealTimers()
  vi.unstubAllGlobals()
  vi.restoreAllMocks()
})

describe("takePairingLink", () => {
  it("strips the fragment before returning the link", () => {
    const history = { replaceState: vi.fn() }
    expect(takePairingLink({ hash: `#p=${ID}.${vector.secret}` }, history)).toEqual({ pairingId: ID, secret: vector.secret })
    expect(history.replaceState).toHaveBeenCalledWith(null, "", "/")
  })

  it("strips and refuses a malformed link without claiming it", () => {
    for (const hash of [`#p=${ID}`, `#p=${ID}.short`, `#p=${ID}.${vector.secret}.x`, `#p=${ID}x.${vector.secret}`]) {
      const history = { replaceState: vi.fn() }
      expect(takePairingLink({ hash }, history)).toBe("invalid")
      expect(history.replaceState).toHaveBeenCalledOnce()
    }
    const history = { replaceState: vi.fn() }
    expect(takePairingLink({ hash: "" }, history)).toBeNull()
    expect(history.replaceState).not.toHaveBeenCalled()
  })
})

describe("claimPairing", () => {
  beforeEach(() => {
    vi.spyOn(base64url, "randomBytes").mockReturnValue(base64url.decodeExact(vector.deviceNonce, 32)!)
  })

  it("claims once and accepts a SAS it derives itself", async () => {
    fetchMock.mockResolvedValueOnce(json({ gatewayNonce: vector.gatewayNonce, sas: vector.sas, claimToken: "T" }))
    await expect(claimPairing({ pairingId: ID, secret: vector.secret }, "iPhone")).resolves.toEqual({ claimToken: "T", sas: vector.sas })
    const [url, init] = fetchMock.mock.calls[0]
    expect(url).toBe("/api/pair/claim")
    expect(JSON.parse(String(init?.body))).toEqual({ pairingId: ID, secret: vector.secret, deviceLabel: "iPhone", deviceNonce: vector.deviceNonce })
  })

  it("refuses a server SAS that does not match", async () => {
    fetchMock.mockResolvedValueOnce(json({ gatewayNonce: vector.gatewayNonce, sas: "000000", claimToken: "T" }))
    await expect(claimPairing({ pairingId: ID, secret: vector.secret }, "iPhone")).rejects.toBeInstanceOf(SasMismatchError)
  })

  it("never resends a claim whose answer was lost", async () => {
    fetchMock.mockRejectedValueOnce(new TypeError("network"))
    await expect(claimPairing({ pairingId: ID, secret: vector.secret }, "iPhone")).rejects.toMatchObject({ status: 0 })
    expect(fetchMock).toHaveBeenCalledOnce()
  })
})

describe("pollPairing", () => {
  it("backs off on 429, keeps one poll in flight and stops at the first ticket", async () => {
    vi.useFakeTimers()
    let inFlight = 0
    let most = 0
    const answers = [new Response("Pairing limit reached", { status: 429 }), json({ state: "pending" }), json({ state: "approved", registrationTicket: "K" }), json({ state: "approved", registrationTicket: "K2" })]
    fetchMock.mockImplementation(async () => {
      inFlight += 1
      most = Math.max(most, inFlight)
      await Promise.resolve()
      inFlight -= 1
      return answers.shift()!
    })
    const result = pollPairing(ID, "T", new AbortController().signal)
    await vi.advanceTimersByTimeAsync(1000)
    await expect(result).resolves.toEqual({ state: "approved", ticket: "K" })
    expect(fetchMock).toHaveBeenCalledTimes(3)
    expect(most).toBe(1)
    expect(answers).toHaveLength(1)
  })

  it("reports a decision and a voided pairing", async () => {
    fetchMock.mockResolvedValueOnce(json({ state: "rejected" }))
    await expect(pollPairing(ID, "T", new AbortController().signal)).resolves.toEqual({ state: "rejected" })
    fetchMock.mockResolvedValueOnce(new Response("Pairing request rejected", { status: 403 }))
    await expect(pollPairing(ID, "T", new AbortController().signal)).resolves.toEqual({ state: "invalid" })
  })

  it("stops when cancelled", async () => {
    const controller = new AbortController()
    fetchMock.mockImplementation((_, init) => new Promise((_, reject) => init?.signal?.addEventListener("abort", () => reject(new DOMException("", "AbortError")))))
    const result = pollPairing(ID, "T", controller.signal)
    controller.abort(new Error("left"))
    await expect(result).rejects.toThrow("left")
  })
})

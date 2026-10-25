import { api, ApiError, type PairingState } from "./api"
import { decodeExact, randomBytes, toBase64Url } from "./base64url"
import { deriveSas } from "./sas"

export type PairingLink = { pairingId: string; secret: string }

/**
 * Reads `#p=<pairingId>.<secret>` once and strips it from the address bar and
 * the history entry before anything else runs. A malformed link is reported
 * without a claim: the gateway voids a pairing on any bad attempt.
 */
export function takePairingLink(location: Pick<Location, "hash">, history: Pick<History, "replaceState">): PairingLink | "invalid" | null {
  if (!location.hash.startsWith("#p=")) return null
  const [pairingId = "", secret = "", ...rest] = location.hash.slice(3).split(".")
  history.replaceState(null, "", "/")
  if (rest.length || !decodeExact(pairingId, 16) || !decodeExact(secret, 32)) return "invalid"
  return { pairingId, secret }
}

export class SasMismatchError extends Error {
  constructor() {
    super("sas-mismatch")
  }
}

/**
 * Claims once. The phone derives the SAS from its own nonce and the secret;
 * a different server copy means someone else holds the link. A lost answer is
 * never resent: the user scans a fresh code instead.
 */
export async function claimPairing(link: PairingLink, deviceLabel: string): Promise<{ claimToken: string; sas: string }> {
  const nonce = randomBytes(32)
  const response = await api.claim({ pairingId: link.pairingId, secret: link.secret, deviceLabel, deviceNonce: toBase64Url(nonce) })
  const gatewayNonce = decodeExact(response.gatewayNonce, 32)
  const secret = decodeExact(link.secret, 32)
  if (!gatewayNonce || !secret || await deriveSas(secret, nonce, gatewayNonce) !== response.sas) throw new SasMismatchError()
  return { claimToken: response.claimToken, sas: response.sas }
}

export type PollResult = { state: "approved"; ticket: string } | { state: Exclude<PairingState, "approved" | "pending"> } | { state: "invalid" }

const sleep = (ms: number, signal: AbortSignal) => new Promise<void>((resolve, reject) => {
  const timer = setTimeout(resolve, ms)
  signal.addEventListener("abort", () => {
    clearTimeout(timer)
    reject(signal.reason)
  }, { once: true })
})

/**
 * One long poll at a time until the desktop decides. The first registration
 * ticket ends polling: every further approved answer would rotate it. 429 and
 * transport errors back off; a 403 means the pairing is gone.
 */
export async function pollPairing(pairingId: string, claimToken: string, signal: AbortSignal): Promise<PollResult> {
  let delay = 1000
  for (;;) {
    // The gateway holds a pending poll for up to 25 s.
    const attempt = new AbortController()
    const abort = () => attempt.abort()
    signal.addEventListener("abort", abort, { once: true })
    const timer = setTimeout(abort, 30_000)
    try {
      const status = await api.pairStatus({ pairingId, claimToken }, attempt.signal)
      if (status.state === "pending") {
        delay = 1000
        continue
      }
      if (status.state === "approved") {
        if (status.registrationTicket) return { state: "approved", ticket: status.registrationTicket }
      } else {
        return { state: status.state }
      }
    } catch (error) {
      if (signal.aborted) throw signal.reason
      if (error instanceof ApiError && error.status === 403) return { state: "invalid" }
    } finally {
      clearTimeout(timer)
      signal.removeEventListener("abort", abort)
    }
    await sleep(delay, signal)
    delay = Math.min(delay * 2, 30_000)
  }
}

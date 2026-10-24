import { ApiError } from "./api"
import { RpcError } from "./rpc"
import { passkeyError } from "./webauthn"

/** An i18n key for a failed request or ceremony. `refused` covers a plain 403. */
export function failureKey(error: unknown, refused = "errors.refused"): string {
  if (error instanceof ApiError) {
    if (error.policyExpired) return "errors.policyExpired"
    if (error.status === 0) return "errors.network"
    if (error.status === 401) return "errors.loginEnded"
    if (error.status === 403) return refused
    if (error.status === 429) return "errors.tooMany"
    return "errors.unavailable"
  }
  if (error instanceof RpcError) return rpcErrorKey(error.code)
  return `passkey.${passkeyError(error)}`
}

const KNOWN = new Set([
  "busy", "session-not-allowed", "session-stopped", "geometry-unavailable", "payload-too-large", "target-not-found",
  "herdr-unavailable", "herdr-environment", "stream-limit", "disconnected", "not-connected", "rate-limited", "outcome-unknown",
  "input-dropped", "input-outcome-unknown", "write-dropped", "write-outcome-unknown", "elevation-required", "elevation-expired",
  "fresh-elevation-required", "scope-not-granted", "owned-elsewhere", "taken-over", "no-agent", "audit-unavailable",
  "stream-stalled", "slow-consumer", "stream-closed", "full-limit", "key-not-allowed", "invalid-params", "stream-not-found",
  "agent-blocked", "git-untrusted", "git-unavailable", "git-not-repository", "git-cursor-invalid", "git-failed",
  "push-unavailable", "push-store-failed", "push-unconfirmed",
])

export function rpcErrorKey(code: string): string {
  return KNOWN.has(code) ? `rpc.${code}` : "rpc.unknown"
}

/** The write may have reached the pane: the user must look before trying again. */
export function outcomeUnknown(code: string): boolean {
  return code === "outcome-unknown" || code === "input-outcome-unknown" || code === "write-outcome-unknown"
}

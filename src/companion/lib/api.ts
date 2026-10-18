import type { Scope } from "./protocol"

/** An HTTP refusal (status ≥ 400) or, with status 0, a request that got no answer. */
export class ApiError extends Error {
  constructor(readonly status: number, readonly body: string) {
    super(status === 0 ? "network" : `${status} ${body}`)
  }
  /** The gateway answers every request this way once its enable window ends. */
  get policyExpired(): boolean {
    return this.status === 403 && this.body === "Policy expired"
  }
}

export type ClaimResponse = { gatewayNonce: string; sas: string; claimToken: string }
export type PairingState = "pending" | "approved" | "rejected" | "expired" | "registering" | "registered"
export type StatusResponse = { state: PairingState; registrationTicket?: string }
export type LoginResponse = { binding: string; deviceId: string; scopes: Scope[] }
export type ElevationResponse = { scope: Scope; until: string }

async function post<T>(path: string, body: unknown, binding?: string, signal?: AbortSignal): Promise<T> {
  const headers: Record<string, string> = { "Content-Type": "application/json" }
  if (binding) headers["X-Yzc-Binding"] = binding
  let response: Response
  try {
    response = await fetch(path, { method: "POST", headers, body: JSON.stringify(body), credentials: "same-origin", cache: "no-store", signal })
  } catch (error) {
    if (signal?.aborted) throw error
    throw new ApiError(0, "")
  }
  if (!response.ok) throw new ApiError(response.status, (await response.text().catch(() => "")).slice(0, 200))
  return (response.status === 204 ? undefined : await response.json()) as T
}

// Pairing secrets, tokens and the binding only ever travel in bodies and
// headers: the gateway refuses any query string on /api.
export const api = {
  claim: (body: { pairingId: string; secret: string; deviceLabel: string; deviceNonce: string }) => post<ClaimResponse>("/api/pair/claim", body),
  pairStatus: (body: { pairingId: string; claimToken: string }, signal?: AbortSignal) => post<StatusResponse>("/api/pair/status", body, undefined, signal),
  registrationOptions: (body: { pairingId: string; registrationTicket: string }) => post<unknown>("/api/pair/webauthn/options", body),
  registrationFinish: (body: { pairingId: string; credential: unknown }) => post<{ deviceId: string }>("/api/pair/webauthn/finish", body),
  loginOptions: () => post<unknown>("/api/auth/options", {}),
  loginFinish: (credential: unknown) => post<LoginResponse>("/api/auth/finish", { credential }),
  elevateOptions: (scope: Scope, binding: string) => post<unknown>("/api/auth/elevate/options", { scope }, binding),
  elevateFinish: (scope: Scope, credential: unknown, binding: string) => post<ElevationResponse>("/api/auth/elevate/finish", { scope, credential }, binding),
  logout: (binding: string) => post<void>("/api/auth/logout", {}, binding),
}

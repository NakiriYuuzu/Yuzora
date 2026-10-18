import { fromBase64Url, toBase64Url } from "./base64url"

type Json = Record<string, unknown>
type Descriptor = { id: string; type: string; transports?: string[] }

export type PasskeyError = "cancelled" | "unsupported" | "exists" | "security" | "failed"

export function passkeysSupported(): boolean {
  return typeof PublicKeyCredential !== "undefined" && typeof navigator.credentials?.create === "function"
}

function descriptors(list: unknown): PublicKeyCredentialDescriptor[] | undefined {
  if (!Array.isArray(list)) return undefined
  return (list as Descriptor[]).map((item) => ({ ...item, id: fromBase64Url(item.id), type: "public-key", transports: item.transports as AuthenticatorTransport[] | undefined }))
}

/** The gateway's flat creation-options JSON with its base64url fields decoded. */
export function creationOptions(json: unknown): PublicKeyCredentialCreationOptions {
  const options = json as Json & { challenge: string; user: Json & { id: string } }
  return {
    ...(options as unknown as PublicKeyCredentialCreationOptions),
    challenge: fromBase64Url(options.challenge),
    user: { ...(options.user as unknown as PublicKeyCredentialUserEntity), id: fromBase64Url(options.user.id) },
    excludeCredentials: descriptors(options.excludeCredentials),
  }
}

export function requestOptions(json: unknown): PublicKeyCredentialRequestOptions {
  const options = json as Json & { challenge: string }
  return {
    ...(options as unknown as PublicKeyCredentialRequestOptions),
    challenge: fromBase64Url(options.challenge),
    allowCredentials: descriptors(options.allowCredentials),
  }
}

/** RegistrationResponseJSON / AuthenticationResponseJSON, built by hand where `toJSON` is missing. */
export function credentialJSON(credential: PublicKeyCredential): Json {
  const response = credential.response as AuthenticatorResponse & Partial<AuthenticatorAttestationResponse & AuthenticatorAssertionResponse>
  const json: Json = typeof credential.toJSON === "function"
    ? { ...(credential.toJSON() as unknown as Json) }
    : {
        id: credential.id,
        rawId: toBase64Url(credential.rawId),
        type: credential.type,
        authenticatorAttachment: credential.authenticatorAttachment,
        clientExtensionResults: credential.getClientExtensionResults(),
        response: response.attestationObject
          ? {
              clientDataJSON: toBase64Url(response.clientDataJSON),
              attestationObject: toBase64Url(response.attestationObject),
              authenticatorData: response.getAuthenticatorData ? toBase64Url(response.getAuthenticatorData()) : undefined,
              publicKey: response.getPublicKey?.() ? toBase64Url(response.getPublicKey()!) : undefined,
              publicKeyAlgorithm: response.getPublicKeyAlgorithm?.(),
              transports: response.getTransports?.(),
            }
          : {
              clientDataJSON: toBase64Url(response.clientDataJSON),
              authenticatorData: toBase64Url(response.authenticatorData!),
              signature: toBase64Url(response.signature!),
              userHandle: response.userHandle ? toBase64Url(response.userHandle) : null,
            },
      }
  if (json.authenticatorAttachment == null) delete json.authenticatorAttachment
  return json
}

export function passkeyError(error: unknown): PasskeyError {
  const name = typeof error === "object" && error !== null && "name" in error ? error.name : ""
  if (name === "NotAllowedError" || name === "AbortError") return "cancelled"
  if (name === "InvalidStateError") return "exists"
  if (name === "NotSupportedError") return "unsupported"
  if (name === "SecurityError") return "security"
  return "failed"
}

// Both must run straight from a tap: Safari refuses a ceremony that is not
// under a user gesture, so callers fetch the options beforehand.
export async function createPasskey(options: unknown): Promise<Json> {
  const credential = await navigator.credentials.create({ publicKey: creationOptions(options) })
  if (!credential) throw new DOMException("No credential", "NotAllowedError")
  return credentialJSON(credential as PublicKeyCredential)
}

export async function getPasskey(options: unknown): Promise<Json> {
  const credential = await navigator.credentials.get({ publicKey: requestOptions(options), mediation: "optional" })
  if (!credential) throw new DOMException("No credential", "NotAllowedError")
  return credentialJSON(credential as PublicKeyCredential)
}

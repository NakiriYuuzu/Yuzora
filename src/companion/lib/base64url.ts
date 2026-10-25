/** Unpadded base64url, the gateway's encoding for ids, nonces, secrets and WebAuthn fields. */
export function toBase64Url(bytes: ArrayBuffer | Uint8Array): string {
  const view = bytes instanceof Uint8Array ? bytes : new Uint8Array(bytes)
  let binary = ""
  for (const byte of view) binary += String.fromCharCode(byte)
  return btoa(binary).replace(/\+/g, "-").replace(/\//g, "_").replace(/=+$/, "")
}

export function fromBase64Url(text: string): Uint8Array<ArrayBuffer> {
  if (!/^[A-Za-z0-9_-]*$/.test(text) || text.length % 4 === 1) throw new Error("invalid base64url")
  return fromBase64(text.replace(/-/g, "+").replace(/_/g, "/"))
}

/** Standard (padded) base64, used by the snapshot chunks. */
export function fromBase64(text: string): Uint8Array<ArrayBuffer> {
  const binary = atob(text)
  const bytes = new Uint8Array(binary.length)
  for (let index = 0; index < binary.length; index += 1) bytes[index] = binary.charCodeAt(index)
  return bytes
}

/** Decodes an unpadded base64url value of exactly `size` bytes, or null. */
export function decodeExact(text: string, size: number): Uint8Array<ArrayBuffer> | null {
  if (text.length !== Math.ceil((size * 8) / 6)) return null
  try {
    const bytes = fromBase64Url(text)
    return bytes.length === size ? bytes : null
  } catch {
    return null
  }
}

export function randomBytes(size: number): Uint8Array<ArrayBuffer> {
  return crypto.getRandomValues(new Uint8Array(size))
}

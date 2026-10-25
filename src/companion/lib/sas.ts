const encoder = new TextEncoder()

/**
 * The gateway's `derive_sas`: HMAC-SHA256(secret, "yz-sas-v1" ‖ deviceNonce ‖
 * gatewayNonce [‖ u32-BE counter]), bytes 0..249 mapped to digits. The phone
 * computes it itself; showing the server's copy would let a relay pass.
 */
export async function deriveSas(secret: Uint8Array<ArrayBuffer>, deviceNonce: Uint8Array, gatewayNonce: Uint8Array): Promise<string> {
  const key = await crypto.subtle.importKey("raw", secret, { name: "HMAC", hash: "SHA-256" }, false, ["sign"])
  let result = ""
  for (let counter = 0; ; counter += 1) {
    const parts = [encoder.encode("yz-sas-v1"), deviceNonce, gatewayNonce]
    if (counter !== 0) parts.push(new Uint8Array([counter >>> 24, (counter >>> 16) & 255, (counter >>> 8) & 255, counter & 255]))
    const message = new Uint8Array(parts.reduce((size, part) => size + part.length, 0))
    let offset = 0
    for (const part of parts) {
      message.set(part, offset)
      offset += part.length
    }
    for (const byte of new Uint8Array(await crypto.subtle.sign("HMAC", key, message))) {
      if (byte >= 250) continue
      result += String(byte % 10)
      if (result.length === 6) return result
    }
  }
}

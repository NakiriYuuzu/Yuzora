// Unicode Cf code points the gateway names explicitly (devices.rs `label_format`),
// plus line/paragraph separators: labels must not hide or reorder approval text.
const FORMAT_RANGES: [number, number][] = [
  [0x00ad, 0x00ad], [0x0600, 0x0605], [0x061c, 0x061c], [0x06dd, 0x06dd], [0x070f, 0x070f],
  [0x0890, 0x0891], [0x08e2, 0x08e2], [0x180e, 0x180e], [0x200b, 0x200f], [0x2028, 0x202e],
  [0x2060, 0x2064], [0x2066, 0x206f], [0xfeff, 0xfeff], [0xfff9, 0xfffb], [0x110bd, 0x110bd],
  [0x110cd, 0x110cd], [0x13430, 0x1343f], [0x1bca0, 0x1bca3], [0x1d173, 0x1d17a],
  [0xe0001, 0xe0001], [0xe0020, 0xe007f],
]
const encoder = new TextEncoder()

export type LabelProblem = "empty" | "too-long" | "edge-space" | "hidden-character"

/**
 * The gateway's `valid_label`, checked before the claim so the phone can say
 * what to fix: a refused claim burns the pairing. Never looser than the server.
 */
export function labelProblem(label: string): LabelProblem | null {
  if (label.length === 0) return "empty"
  if (encoder.encode(label).length > 80) return "too-long"
  // JS trims the same White_Space set as Rust's `trim`, except U+0085, a control below.
  if (label.trim() !== label) return "edge-space"
  for (const char of label) {
    const code = char.codePointAt(0)!
    if (code <= 0x1f || (code >= 0x7f && code <= 0x9f)) return "hidden-character"
    if (FORMAT_RANGES.some(([low, high]) => code >= low && code <= high)) return "hidden-character"
  }
  return null
}

/** A readable default from the browser; every value passes `labelProblem`. */
export function defaultLabel(userAgent: string): string {
  if (/iPhone/.test(userAgent)) return "iPhone"
  if (/iPad/.test(userAgent)) return "iPad"
  if (/Android/.test(userAgent)) return "Android"
  return "Phone"
}

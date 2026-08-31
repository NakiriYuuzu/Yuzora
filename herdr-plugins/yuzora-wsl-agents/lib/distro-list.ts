function looksUtf16Le(bytes: Uint8Array): boolean {
  if (bytes.length >= 2 && bytes[0] === 0xff && bytes[1] === 0xfe) return true
  const limit = Math.min(bytes.length, 64)
  for (let i = 1; i < limit; i += 2) {
    if (bytes[i] === 0) return true
  }
  return false
}

function decodeUtf16Le(bytes: Uint8Array): string {
  const start = bytes.length >= 2 && bytes[0] === 0xff && bytes[1] === 0xfe ? 2 : 0
  const units: number[] = []
  for (let i = start; i + 1 < bytes.length; i += 2) {
    units.push(bytes[i] | (bytes[i + 1] << 8))
  }
  return String.fromCharCode(...units)
}

function trimDistroLine(line: string): string {
  // WSL --list --quiet may pad UTF-16 output with NULs that must be stripped.
  // eslint-disable-next-line no-control-regex -- NUL is part of the WSL list encoding.
  return line.replace(/^[\uFEFF\u0000 \t\r]+|[\uFEFF\u0000 \t\r]+$/g, "")
}

/** Match Yuzora `decode_wsl_list_output` / `wsl.exe --list --quiet`. */
export function decodeWslListOutput(bytes: Uint8Array): string[] {
  const text = looksUtf16Le(bytes)
    ? decodeUtf16Le(bytes)
    : new TextDecoder("utf-8", { fatal: false }).decode(bytes)
  return text
    .split(/\r?\n/)
    .map(trimDistroLine)
    .filter((line) => line.length > 0)
}

export function assertSafeDistroName(name: string): string {
  const trimmed = name.trim()
  if (!trimmed) throw new Error("distro name is empty")
  if (/[\\/]/.test(trimmed) || trimmed.includes("..")) {
    throw new Error(`unsafe distro name: ${JSON.stringify(trimmed)}`)
  }
  return trimmed
}

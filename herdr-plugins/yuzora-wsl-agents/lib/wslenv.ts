const SOCKET_NAME = "HERDR_SOCKET_PATH"

function entryName(entry: string): string {
  const trimmed = entry.trim()
  if (!trimmed) return ""
  const slash = trimmed.indexOf("/")
  return (slash === -1 ? trimmed : trimmed.slice(0, slash)).toUpperCase()
}

export function splitWslenv(value: string | null | undefined): string[] {
  if (!value) return []
  return value.split(":").map((part) => part.trim()).filter((part) => part.length > 0)
}

/** Drop every inherited HERDR_SOCKET_PATH entry, including any WSLENV flags. */
export function denyHerdrSocketPathEntries(entries: string[]): string[] {
  return entries.filter((entry) => entryName(entry) !== SOCKET_NAME)
}

export function dedupeWslenv(entries: string[]): string[] {
  const map = new Map<string, string>()
  for (const entry of entries) {
    const name = entryName(entry)
    if (!name) continue
    map.set(name, entry)
  }
  return [...map.values()]
}

export function mergeWin32ToWslEnv(
  existing: string | null | undefined,
  additions: readonly string[]
): string {
  const kept = denyHerdrSocketPathEntries(splitWslenv(existing))
  return dedupeWslenv([...kept, ...additions]).join(":")
}

export function mergeReporterChildWslenv(
  existing: string | null | undefined,
  childEntry: string
): string {
  const kept = denyHerdrSocketPathEntries(splitWslenv(existing))
  return dedupeWslenv([...kept, childEntry]).join(":")
}

export function containsRawSocketEntry(value: string | null | undefined): boolean {
  return splitWslenv(value).some((entry) => entryName(entry) === SOCKET_NAME)
}

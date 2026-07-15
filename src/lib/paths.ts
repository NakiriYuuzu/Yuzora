// cwd 必須是絕對路徑才可用來 spawn agent：posix 以 "/" 開頭、Windows 磁碟機開頭
// （C:\ 或 C:/），或 Windows 的 UNC／verbatim 前綴（\\server\share、\\?\C:\…）。
// 後者是 std::fs::canonicalize 在 Windows 回傳 workspacePath 的實際形式，漏掉會讓
// AgentZone 在 Windows 完全無法新增 session。
export function isAbsolutePath(path: string | null | undefined): path is string {
  if (!path) return false
  return path.startsWith("/") || path.startsWith("\\\\") || /^[A-Za-z]:[\\/]/.test(path)
}

export function firstAbsolutePath(...paths: (string | null | undefined)[]): string | null {
  return paths.find(isAbsolutePath) ?? null
}

/**
 * Derive user-facing text from a canonical workspace path without changing
 * the operational path kept in state or passed to IPC.
 */
export function workspacePathForDisplay(path: string): string {
  if (/^[\\/]{2}\?[\\/]UNC[\\/]/i.test(path)) {
    const separator = path.startsWith("\\") ? "\\" : "/"
    return separator + separator + path.slice(8)
  }
  if (/^[\\/]{2}\?[\\/][A-Za-z]:[\\/]/.test(path)) return path.slice(4)
  return path
}

export function workspacePathBasename(path: string): string {
  const displayPath = workspacePathForDisplay(path)
  const withoutTrailingSeparators = displayPath.replace(/[\\/]+$/, "")
  if (!withoutTrailingSeparators) return displayPath
  return withoutTrailingSeparators.split(/[\\/]+/).filter(Boolean).at(-1)
    ?? displayPath
}

/**
 * Comparison-only identity for canonical paths. Windows drive and UNC paths
 * are case-insensitive and extended/non-extended aliases compare equal;
 * POSIX paths remain case-sensitive.
 */
export function canonicalPathKey(path: string): string {
  let normalized = path.replace(/\\/g, "/")
  if (normalized.toLowerCase().startsWith("//?/unc/")) {
    normalized = "//" + normalized.slice("//?/UNC/".length)
  } else if (/^\/\/\?\/[A-Za-z]:\//.test(normalized)) {
    normalized = normalized.slice("//?/".length)
  }
  const isGenericDoubleSlashPath = normalized.startsWith("//?/")
  const isUnc = normalized.startsWith("//") && !isGenericDoubleSlashPath
  if (!isUnc && !isGenericDoubleSlashPath) {
    normalized = normalized.replace(/\/{2,}/g, "/")
  }
  while (
    normalized.length > 1
    && normalized.endsWith("/")
    && !/^[A-Za-z]:\/$/.test(normalized)
  ) {
    normalized = normalized.slice(0, -1)
  }
  return /^[A-Za-z]:\//.test(normalized) || isUnc
    ? normalized.toLowerCase()
    : normalized
}

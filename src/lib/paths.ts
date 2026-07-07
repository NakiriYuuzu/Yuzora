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

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

/** True for operational paths whose syntax unambiguously identifies Windows. */
export function isWindowsPath(path: string): boolean {
  return /^[A-Za-z]:(?:[\\/]|$)/.test(path)
    || /^[\\/]{2}\?[\\/](?:UNC[\\/]|[A-Za-z]:[\\/])/i.test(path)
    || path.startsWith("\\\\")
}

function normalizedPath(path: string, windows: boolean): string {
  let normalized = path.replace(/\\/g, "/")
  if (normalized.toLowerCase().startsWith("//?/unc/")) {
    normalized = "//" + normalized.slice("//?/UNC/".length)
  } else if (/^\/\/\?\/[A-Za-z]:\//.test(normalized)) {
    normalized = normalized.slice("//?/".length)
  }

  // POSIX reserves exactly two leading slashes as an implementation-defined
  // namespace. Three or more leading slashes normalize to the ordinary `/`
  // namespace. Windows UNC paths keep exactly two leading slashes.
  if (!windows && /^\/{3,}/.test(normalized)) {
    normalized = "/" + normalized.replace(/^\/+/, "").replace(/\/{2,}/g, "/")
  } else if (normalized.startsWith("//")) {
    normalized = "//" + normalized.slice(2).replace(/\/{2,}/g, "/")
  } else {
    normalized = normalized.replace(/\/{2,}/g, "/")
  }

  while (
    normalized.length > 1
    && normalized !== "//"
    && normalized.endsWith("/")
    && !(windows && /^[A-Za-z]:\/$/.test(normalized))
  ) {
    normalized = normalized.slice(0, -1)
  }
  return normalized
}

/**
 * Comparison-only identity for canonical paths. Unambiguous Windows drive,
 * backslash-UNC, and verbatim paths are case-insensitive; POSIX paths,
 * including implementation-defined `//...` paths, remain case-sensitive.
 * `style: "windows"` is reserved for matching an LSP `file://host/...` path
 * against a known Windows UNC operational path.
 */
export function canonicalPathKey(
  path: string,
  style: "auto" | "windows" = "auto"
): string {
  const windows = style === "windows" || isWindowsPath(path)
  const normalized = normalizedPath(path, windows)
  return windows ? normalized.toLowerCase() : normalized
}

function comparisonKeys(left: string, right: string): [string, string, boolean] {
  const windows = isWindowsPath(left) || isWindowsPath(right)
  return [
    canonicalPathKey(left, windows ? "windows" : "auto"),
    canonicalPathKey(right, windows ? "windows" : "auto"),
    windows,
  ]
}

/** Compare path identity without rewriting either operational path. */
export function samePathIdentity(left: string, right: string): boolean {
  const [leftKey, rightKey] = comparisonKeys(left, right)
  return leftKey === rightKey
}

type PathNamespace =
  | "windows-drive"
  | "windows-unc"
  | "windows-relative"
  | "posix-root"
  | "posix-double-root"
  | "posix-relative"

interface SegmentedPath {
  namespace: PathNamespace
  anchor: string
  segments: string[]
  windows: boolean
}

/**
 * Split a comparison path without losing the original spelling of descendant
 * segments. This deliberately avoids deriving character offsets from a
 * lower-cased key: Unicode case folds such as `İ` → `i\u0307` can change UTF-16
 * length even though the two Windows identities compare equal.
 */
function segmentedPath(path: string, windows: boolean): SegmentedPath {
  const normalized = normalizedPath(path, windows)

  if (windows) {
    const drive = normalized.match(/^([A-Za-z]:)(?:\/(.*))?$/)
    if (drive) {
      return {
        namespace: "windows-drive",
        anchor: drive[1],
        segments: (drive[2] ?? "").split("/").filter(Boolean),
        windows,
      }
    }
    if (normalized.startsWith("//")) {
      return {
        namespace: "windows-unc",
        anchor: "//",
        segments: normalized.slice(2).split("/").filter(Boolean),
        windows,
      }
    }
    return {
      namespace: "windows-relative",
      anchor: "",
      segments: normalized.split("/").filter(Boolean),
      windows,
    }
  }

  if (normalized.startsWith("//")) {
    return {
      namespace: "posix-double-root",
      anchor: "//",
      segments: normalized.slice(2).split("/").filter(Boolean),
      windows,
    }
  }
  if (normalized.startsWith("/")) {
    return {
      namespace: "posix-root",
      anchor: "/",
      segments: normalized.slice(1).split("/").filter(Boolean),
      windows,
    }
  }
  return {
    namespace: "posix-relative",
    anchor: "",
    segments: normalized.split("/").filter(Boolean),
    windows,
  }
}

function comparisonSegment(value: string, windows: boolean): string {
  return windows ? value.toLowerCase() : value
}

function relativeSuffix(root: string, path: string): string | null {
  const windows = isWindowsPath(root) || isWindowsPath(path)
  const rootParts = segmentedPath(root, windows)
  const pathParts = segmentedPath(path, windows)
  if (rootParts.namespace !== pathParts.namespace) return null
  if (
    comparisonSegment(rootParts.anchor, windows)
    !== comparisonSegment(pathParts.anchor, windows)
  ) return null
  if (rootParts.segments.length > pathParts.segments.length) return null

  for (let index = 0; index < rootParts.segments.length; index += 1) {
    if (
      comparisonSegment(rootParts.segments[index], windows)
      !== comparisonSegment(pathParts.segments[index], windows)
    ) return null
  }

  return pathParts.segments.slice(rootParts.segments.length).join("/")
}

function preferredSeparator(path: string): "/" | "\\" {
  // Prefer the separator already present in the operational path so joins and
  // rebases preserve Windows/POSIX form. Mixed-separator inputs still work
  // because identity uses canonicalPathKey.
  if (path.includes("\\") && !path.includes("/")) return "\\"
  if (/^[A-Za-z]:(?:\\|$)/.test(path) || path.startsWith("\\\\")) return "\\"
  if (/^[A-Za-z]:\//.test(path)) return "/"
  return "/"
}

function stripTrailingSeparators(path: string): string {
  if (!path) return path
  // Keep POSIX root, drive root, and UNC share roots intact.
  if (path === "/" || path === "\\") return path
  if (/^[A-Za-z]:[\\/]?$/.test(path)) return path
  if (/^[\\/]{2}[^\\/]+[\\/][^\\/]+[\\/]?$/.test(path) && !path.startsWith("\\\\?\\")) {
    return path.replace(/[\\/]+$/, "") || path
  }
  const trimmed = path.replace(/[\\/]+$/, "")
  return trimmed.length > 0 ? trimmed : path
}

function splitSegments(path: string): string[] {
  const display = workspacePathForDisplay(path)
  if (display === "/" || display === "\\") return []
  if (/^[A-Za-z]:[\\/]?$/.test(display)) return []
  return display.replace(/^[\\/]+/, "").split(/[\\/]+/).filter(Boolean)
}

/**
 * Join a directory and a child name using the directory's native separator.
 * Does not rewrite the operational path to a display form.
 */
export function nativePathJoin(dir: string, name: string): string {
  if (!name) return dir
  if (!dir) return name
  const sep = preferredSeparator(dir)
  const child = name.replace(/^[\\/]+/, "")
  // Drive roots like C: or C:\ already imply the root separator.
  if (/^[A-Za-z]:[\\/]?$/.test(dir)) {
    return `${dir.match(/^[A-Za-z]:/)![0]}${sep}${child}`
  }
  if (dir === "//") return `//${child}`
  if (/^[\\/]{2}\?[\\/][A-Za-z]:[\\/]?$/.test(dir)) {
    const match = dir.match(/^[\\/]{2}\?[\\/][A-Za-z]:/)!
    return `${match[0]}${sep}${child}`
  }
  if (dir === "/" || dir === "\\") return `${dir}${child}`
  const base = stripTrailingSeparators(dir)
  return `${base}${sep}${child}`
}

/**
 * Parent directory of a local/native filesystem path.
 * Supports POSIX, drive letters, UNC shares, and Windows verbatim prefixes.
 */
export function nativePathParent(path: string): string {
  if (!path) return path
  const sep = preferredSeparator(path)
  const trimmed = stripTrailingSeparators(path)

  // POSIX root.
  if (trimmed === "/" || trimmed === "\\") return "/"

  // Drive root (C: or C:\).
  if (/^[A-Za-z]:[\\/]?$/.test(trimmed) || /^[\\/]{2}\?[\\/][A-Za-z]:[\\/]?$/.test(trimmed)) {
    return /^[\\/]{2}\?/.test(path)
      ? `${path.startsWith("/") ? "//?/" : "\\\\?\\"}${trimmed.match(/[A-Za-z]/)![0]}:${sep}`
      : `${trimmed.match(/^[A-Za-z]/)![0]}:${sep}`
  }

  // UNC share root (\\server\share) has no parent above the share.
  const unc = trimmed.match(/^([\\/]{2}[^\\/]+[\\/][^\\/]+)/)
  if (unc && splitSegments(trimmed).length <= 2 && !/^[\\/]{2}\?/.test(trimmed)) {
    return unc[1]
  }
  const uncVerbatim = trimmed.match(/^([\\/]{2}\?[\\/]UNC[\\/][^\\/]+[\\/][^\\/]+)/i)
  if (uncVerbatim && splitSegments(workspacePathForDisplay(trimmed)).length <= 2) {
    return uncVerbatim[1]
  }

  const lastSlash = Math.max(trimmed.lastIndexOf("/"), trimmed.lastIndexOf("\\"))
  if (lastSlash < 0) return trimmed

  // Keep drive root when trimming the final segment of C:\foo.
  if (/^[A-Za-z]:[\\/]/.test(trimmed) && lastSlash === 2) {
    return trimmed.slice(0, 3)
  }
  // Verbatim drive root: \\?\C:\foo → \\?\C:\
  if (/^[\\/]{2}\?[\\/][A-Za-z]:[\\/]/.test(trimmed)) {
    const rootEnd = trimmed.search(/[A-Za-z]:[\\/]/) + 2
    if (lastSlash <= rootEnd) return trimmed.slice(0, rootEnd + 1)
  }
  // UNC: \\server\share\foo → \\server\share
  if (unc && lastSlash <= unc[1].length) return unc[1]
  if (uncVerbatim && lastSlash <= uncVerbatim[1].length) return uncVerbatim[1]

  // POSIX absolute: /a → /
  if (trimmed.startsWith("/") && lastSlash === 0) return "/"

  return trimmed.slice(0, lastSlash)
}

/** True when `path` is exactly `root` or a descendant of `root`. */
export function isSameOrDescendantPath(root: string, path: string): boolean {
  return relativeSuffix(root, path) !== null
}

/**
 * Rewrite a path that equals or lives under `fromRoot` so it lives under
 * `toRoot`, preserving the operational separator style of `toRoot` and the
 * original casing of every descendant segment.
 */
export function rebasePath(fromRoot: string, toRoot: string, path: string): string | null {
  const relative = relativeSuffix(fromRoot, path)
  if (relative === null) return null
  if (!relative) return toRoot

  const sep = preferredSeparator(toRoot)
  return nativePathJoin(toRoot, relative.replace(/\//g, sep))
}

/**
 * Repo/workspace-relative form with forward slashes for Git and UI relative
 * paths. Membership is compared canonically, while returned casing comes from
 * the original operational path.
 */
export function relativePathWithin(root: string, path: string): string | null {
  return relativeSuffix(root, path)
}

/**
 * Local filesystem path helpers. Remote SFTP paths remain POSIX and must not
 * call these functions.
 */
export const localPathJoin = nativePathJoin
export const localPathParent = nativePathParent

/** Fail-early leaf check matching the backend SafeLeafName contract. */
export function isSafeLeafName(name: string): boolean {
  if (!name || name === "." || name === "..") return false
  if (name.includes("\0") || name.includes("/") || name.includes("\\")) return false
  if (name.startsWith(":") || /^[A-Za-z]:/.test(name)) return false
  if (name.startsWith("//") || name.startsWith("\\\\")) return false
  return true
}

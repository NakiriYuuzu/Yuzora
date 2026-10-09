import { toast } from "sonner"

import { parseRuntimeScope } from "@/lib/herdrProvider"
import { wslPath } from "@/lib/hostIpc"
import i18n from "@/lib/i18n"
import { isWindowsPlatform } from "@/lib/platform"
import { LOCAL_HOST_ID, parseRemoteFilePath } from "@/lib/runtimeIdentity"
import { useHostStore } from "@/state/hostStore"
import type { TerminalDropTarget } from "./terminalDropTargets"

export type ShellFlavor = "posix" | "windows"

export type TerminalPathPasteErrorCode = "empty" | "not-writable" | "remote-terminal" | "host-mismatch" | "unsafe-name"

export class TerminalPathPasteError extends Error {
  constructor(readonly code: TerminalPathPasteErrorCode) {
    super(code)
    this.name = "TerminalPathPasteError"
  }
}

const POSIX_SAFE = /[A-Za-z0-9_\-./:@%+,=]/
const WINDOWS_SAFE = /^[A-Za-z0-9_\-.:\\/]*$/
// File names are untrusted input. C0/C1 controls (ESC, CR, LF, ^C, CSI...) act
// as keystrokes once a shell or TUI ignores bracketed paste, so no quoting
// makes them inert: such names are refused.
// eslint-disable-next-line no-control-regex
const CONTROL = /[\u0000-\u001f\u007f-\u009f]/
// `$` and backtick stay live inside PowerShell double quotes, and PowerShell
// also ends a double-quoted string at the smart quotes U+201C-U+201E; cmd
// expands `%NAME%` (and `!NAME!` with delayed expansion) inside them. No quoting
// is literal in both cmd and PowerShell, so a Windows name with them is refused.
const WINDOWS_UNQUOTABLE = /[$`"“”„%!]/
const WINDOWS_LIKE_HOST_PATH = /^[A-Za-z]:[\\/]|^\\\\/

function quotePosix(path: string): string {
  return Array.from(path, (char) => (char.charCodeAt(0) > 0x7f || POSIX_SAFE.test(char) ? char : `\\${char}`)).join("")
}

function quoteWindows(path: string): string {
  if (WINDOWS_UNQUOTABLE.test(path)) throw new TerminalPathPasteError("unsafe-name")
  return WINDOWS_SAFE.test(path) ? path : `"${path}"`
}

function quoteOne(path: string, flavor: ShellFlavor): string {
  if (CONTROL.test(path)) throw new TerminalPathPasteError("unsafe-name")
  return flavor === "windows" ? quoteWindows(path) : quotePosix(path)
}

/**
 * Joined by spaces with one trailing space so consecutive drops stay separated.
 * Throws `unsafe-name` for a name that no quoting makes inert.
 */
export function quotePathsForShell(paths: string[], flavor: ShellFlavor): string {
  return paths.map((path) => `${quoteOne(path, flavor)} `).join("")
}

export async function pastePathsIntoTerminal(target: TerminalDropTarget, paths: string[]): Promise<void> {
  if (paths.length === 0) throw new TerminalPathPasteError("empty")
  if (!target.canWrite()) throw new TerminalPathPasteError("not-writable")
  const hostId = parseRuntimeScope(target.scope).hostId
  const config = hostId === LOCAL_HOST_ID ? undefined : useHostStore.getState().configs[hostId]
  let text = ""
  for (const path of paths) {
    const remote = parseRemoteFilePath(path)
    let quoted: string
    if (hostId === LOCAL_HOST_ID) {
      if (remote) throw new TerminalPathPasteError("host-mismatch")
      quoted = quoteOne(path, isWindowsPlatform() ? "windows" : "posix")
    } else if (remote) {
      if (remote.hostId !== hostId) throw new TerminalPathPasteError("host-mismatch")
      quoted = quoteOne(remote.path, WINDOWS_LIKE_HOST_PATH.test(remote.path) ? "windows" : "posix")
    } else if (config?.kind === "wsl" && config.distro) {
      quoted = quoteOne(await wslPath(hostId, config.distro, path), "posix")
    } else {
      throw new TerminalPathPasteError("remote-terminal")
    }
    text += `${quoted} `
  }
  // The WSL conversion awaited; the leaf may have stopped or unmounted since.
  if (!target.canWrite()) throw new TerminalPathPasteError("not-writable")
  await target.paste(text)
  target.focus()
}

export function notifyTerminalPathPasteError(error: unknown): void {
  if (error instanceof TerminalPathPasteError) {
    toast.error(i18n.t(`pathDrop.${error.code}`, { ns: "terminal" }))
    return
  }
  toast.error(error instanceof Error ? error.message : String(error))
}

import { beforeEach, expect, it, vi } from "vitest"

const platform = vi.hoisted(() => ({ windows: false }))
const ipc = vi.hoisted(() => ({ wslPath: vi.fn() }))
const toastMock = vi.hoisted(() => ({ error: vi.fn() }))

vi.mock("@/lib/platform", () => ({ isTauri: () => true, isWindowsPlatform: () => platform.windows }))
vi.mock("@/lib/hostIpc", () => ({ wslPath: ipc.wslPath }))
vi.mock("sonner", () => ({ toast: { error: toastMock.error } }))

import { remoteFilePath, runtimeKey } from "@/lib/runtimeIdentity"
import { useHostStore } from "@/state/hostStore"
import type { TerminalDropTarget } from "./terminalDropTargets"
import {
  notifyTerminalPathPasteError,
  pastePathsIntoTerminal,
  quotePathsForShell,
  TerminalPathPasteError,
} from "./terminalPathPaste"

function fakeTarget(scope: string, writable = true) {
  const calls: string[] = []
  const target: TerminalDropTarget = {
    scope,
    canWrite: () => writable,
    paste: async (text) => { calls.push(`paste:${text}`) },
    focus: () => { calls.push("focus") },
  }
  return { target, calls }
}

const sshScope = runtimeKey({ hostId: "ssh-1", sessionName: "main" })
const wslScope = runtimeKey({ hostId: "wsl-1", sessionName: "main" })

beforeEach(() => {
  platform.windows = false
  ipc.wslPath.mockReset()
  toastMock.error.mockReset()
  useHostStore.setState({
    configs: {
      "wsl-1": { hostId: "wsl-1", label: "WSL", kind: "wsl", distro: "Ubuntu", helper: "", binary: "" },
      "ssh-1": { hostId: "ssh-1", label: "SSH", kind: "ssh", helper: "", binary: "" },
    },
  })
})

it("quotes posix paths: spaces, quotes, $, command substitution, CJK", () => {
  expect(quotePathsForShell(["/a b/c"], "posix")).toBe("/a\\ b/c ")
  expect(quotePathsForShell(["/it's"], "posix")).toBe("/it\\'s ")
  expect(quotePathsForShell(["/$HOME/x"], "posix")).toBe("/\\$HOME/x ")
  expect(quotePathsForShell(["/a$(rm -rf ~)`id`;|&b"], "posix")).toBe("/a\\$\\(rm\\ -rf\\ \\~\\)\\`id\\`\\;\\|\\&b ")
  expect(quotePathsForShell(["/文件/資料.txt"], "posix")).toBe("/文件/資料.txt ")
  expect(quotePathsForShell(["/a", "/b c"], "posix")).toBe("/a /b\\ c ")
})

it("refuses names whose control characters would act as keystrokes", () => {
  // A split bracketed-paste end marker plus ^C and a newline-terminated command.
  const splitMarker = "/tmp/a\x1b[2\x1b[201~01~\x03rm -rf ~\n"
  for (const name of [splitMarker, "/a\nb", "/a\rb", "/a\x1bb", "/a\x7fb", "/a\u009bb"]) {
    for (const flavor of ["posix", "windows"] as const) {
      expect(() => quotePathsForShell([name], flavor)).toThrow(expect.objectContaining({ code: "unsafe-name" }))
    }
  }
})

it("quotes windows paths only when needed and refuses names PowerShell would expand", () => {
  expect(quotePathsForShell(["C:\\Users\\me\\a.txt"], "windows")).toBe("C:\\Users\\me\\a.txt ")
  expect(quotePathsForShell(["C:\\My Files\\a.txt", "D:\\b"], "windows")).toBe('"C:\\My Files\\a.txt" D:\\b ')
  expect(() => quotePathsForShell(["C:\\x$(calc).txt"], "windows")).toThrow(expect.objectContaining({ code: "unsafe-name" }))
  expect(() => quotePathsForShell(["C:\\a`nb.txt"], "windows")).toThrow(expect.objectContaining({ code: "unsafe-name" }))
  // PowerShell treats smart double quotes as ordinary ones: x”;calc;“y would run calc.
  for (const quote of ["\u201c", "\u201d", "\u201e"]) {
    expect(() => quotePathsForShell([`C:\\x${quote};calc;${quote}y.txt`], "windows")).toThrow(expect.objectContaining({ code: "unsafe-name" }))
  }
  // Other non-ASCII names still paste.
  expect(quotePathsForShell(["C:\\資料\\報告.txt"], "windows")).toBe('"C:\\資料\\報告.txt" ')
})

it("pastes nothing when any dropped name is unsafe", async () => {
  const { target, calls } = fakeTarget("main")
  await expect(pastePathsIntoTerminal(target, ["/ok", "/bad\nname"])).rejects.toMatchObject({ code: "unsafe-name" })
  ipc.wslPath.mockResolvedValue("/mnt/c/bad\x1bname")
  const wsl = fakeTarget(wslScope)
  await expect(pastePathsIntoTerminal(wsl.target, ["C:\\bad"])).rejects.toMatchObject({ code: "unsafe-name" })
  expect([...calls, ...wsl.calls]).toEqual([])
})

it("pastes local paths with posix quoting, then focuses", async () => {
  const { target, calls } = fakeTarget("main")
  await pastePathsIntoTerminal(target, ["/tmp/a b.txt"])
  expect(calls).toEqual(["paste:/tmp/a\\ b.txt ", "focus"])
})

it("uses windows quoting for a local terminal on Windows", async () => {
  platform.windows = true
  const { target, calls } = fakeTarget("main")
  await pastePathsIntoTerminal(target, ["C:\\My Files\\a.txt"])
  expect(calls[0]).toBe('paste:"C:\\My Files\\a.txt" ')
})

it("converts local paths for a WSL terminal through wslPath", async () => {
  ipc.wslPath.mockResolvedValue("/mnt/c/My Files/a.txt")
  const { target, calls } = fakeTarget(wslScope)
  await pastePathsIntoTerminal(target, ["C:\\My Files\\a.txt"])
  expect(ipc.wslPath).toHaveBeenCalledWith("wsl-1", "Ubuntu", "C:\\My Files\\a.txt")
  expect(calls[0]).toBe("paste:/mnt/c/My\\ Files/a.txt ")
})

it("rejects local files for an SSH terminal", async () => {
  const { target, calls } = fakeTarget(sshScope)
  await expect(pastePathsIntoTerminal(target, ["/tmp/a"])).rejects.toMatchObject({ code: "remote-terminal" })
  expect(calls).toEqual([])
})

it("rejects remote files of another host", async () => {
  const { target, calls } = fakeTarget(sshScope)
  const other = remoteFilePath("ssh-2", "/srv/a", "/srv")
  await expect(pastePathsIntoTerminal(target, [other])).rejects.toMatchObject({ code: "host-mismatch" })
  await expect(pastePathsIntoTerminal(fakeTarget("main").target, [other])).rejects.toMatchObject({ code: "host-mismatch" })
  expect(calls).toEqual([])
})

it("pastes the host path of a same-host remote file", async () => {
  const { target, calls } = fakeTarget(sshScope)
  await pastePathsIntoTerminal(target, [remoteFilePath("ssh-1", "/srv/my app/a.txt", "/srv")])
  expect(calls[0]).toBe("paste:/srv/my\\ app/a.txt ")
  const win = fakeTarget(sshScope)
  await pastePathsIntoTerminal(win.target, [remoteFilePath("ssh-1", "C:\\My Dir\\a.txt", "C:\\My Dir")])
  expect(win.calls[0]).toBe('paste:"C:\\My Dir\\a.txt" ')
})

it("does not paste into a terminal that cannot write, or with no paths", async () => {
  const readOnly = fakeTarget("main", false)
  await expect(pastePathsIntoTerminal(readOnly.target, ["/a"])).rejects.toMatchObject({ code: "not-writable" })
  await expect(pastePathsIntoTerminal(fakeTarget("main").target, [])).rejects.toMatchObject({ code: "empty" })
  expect(readOnly.calls).toEqual([])
})

it("re-checks writability after the WSL conversion awaited", async () => {
  let writable = true
  const pasted: string[] = []
  const target: TerminalDropTarget = { scope: wslScope, canWrite: () => writable, paste: async (text) => { pasted.push(text) }, focus: () => {} }
  ipc.wslPath.mockImplementation(async () => {
    writable = false
    return "/mnt/c/a"
  })
  await expect(pastePathsIntoTerminal(target, ["C:\\a"])).rejects.toMatchObject({ code: "not-writable" })
  expect(pasted).toEqual([])
})

it("notifies with localized text for known errors and the message for unknown ones", () => {
  notifyTerminalPathPasteError(new TerminalPathPasteError("not-writable"))
  expect(toastMock.error).toHaveBeenLastCalledWith(expect.not.stringContaining("pathDrop."))
  notifyTerminalPathPasteError(new Error("boom"))
  expect(toastMock.error).toHaveBeenLastCalledWith("boom")
})

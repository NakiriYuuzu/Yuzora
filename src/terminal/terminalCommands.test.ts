import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"
import { clearMocks, mockIPC } from "@tauri-apps/api/mocks"

import { useTerminalStore } from "@/state/terminalStore"
import {
  clearTerminalBuffer,
  closeTerminal,
  copyTerminalSelection,
  pasteTerminalClipboard,
  splitTerminal,
  type TerminalCommandTarget,
} from "./terminalCommands"
import { registerTerminalView, type TerminalViewHandle } from "./terminalViewRegistry"

vi.mock("@/app/workbench/settingsStorage", () => ({
  loadTerminalSettings: () => ({ shellPath: "", shellArgs: "" }),
}))

const target: TerminalCommandTarget = {
  workspacePath: "/workspace",
  paneId: "pane-clicked",
  sessionId: "session-clicked",
}

let unregisterView: (() => void) | null = null

function seedTarget(activePaneId: string | null = target.paneId): void {
  useTerminalStore.setState({
    sessions: {
      [target.sessionId]: {
        sessionId: target.sessionId,
        title: "Terminal 1",
        workspace: target.workspacePath,
        shell: "",
        cols: 80,
        rows: 24,
      },
    },
    layouts: {
      [target.workspacePath]: {
        panes: [{ paneId: target.paneId, sessionId: target.sessionId }],
        activePaneId,
        splitDirection: null,
      },
    },
  })
}

function registerView(overrides: Partial<TerminalViewHandle> = {}): TerminalViewHandle {
  const view: TerminalViewHandle = {
    hasSelection: vi.fn(() => true),
    getSelection: vi.fn(() => "selected output"),
    isReady: vi.fn(() => true),
    paste: vi.fn(async () => undefined),
    clear: vi.fn(),
    ...overrides,
  }
  unregisterView = registerTerminalView(target.sessionId, view)
  return view
}

beforeEach(() => {
  clearMocks()
  useTerminalStore.getState().reset()
})

afterEach(() => {
  unregisterView?.()
  unregisterView = null
})

describe("terminalCommands", () => {
  it("copies selection, pastes through xterm, and clears the xterm buffer", async () => {
    seedTarget()
    const view = registerView()
    const calls: Array<{ cmd: string; args: unknown }> = []
    mockIPC((cmd, args) => {
      calls.push({ cmd, args })
      if (cmd === "plugin:clipboard-manager|read_text") return "clipboard text"
      return undefined
    })

    expect(await copyTerminalSelection(target)).toBe("completed")
    expect(calls.find((call) => call.cmd === "plugin:clipboard-manager|write_text")?.args)
      .toEqual({ text: "selected output" })

    expect(await pasteTerminalClipboard(target)).toBe("completed")
    expect(view.paste).toHaveBeenCalledWith("clipboard text")

    expect(clearTerminalBuffer(target)).toBe("completed")
    expect(view.clear).toHaveBeenCalledTimes(1)
  })

  it("cancels an empty clipboard without reporting paste success", async () => {
    seedTarget("unrelated-active-pane")
    const view = registerView()
    mockIPC((cmd) => cmd === "plugin:clipboard-manager|read_text" ? "" : undefined)

    expect(await pasteTerminalClipboard(target)).toBe("cancelled")
    expect(view.paste).not.toHaveBeenCalled()
  })

  it("cancels before reading the clipboard when the clicked PTY is not ready", async () => {
    seedTarget("unrelated-active-pane")
    const view = registerView({ isReady: vi.fn(() => false) })
    const calls: string[] = []
    mockIPC((cmd) => {
      calls.push(cmd)
      return "clipboard"
    })

    expect(await pasteTerminalClipboard(target)).toBe("cancelled")
    expect(calls).not.toContain("plugin:clipboard-manager|read_text")
    expect(view.paste).not.toHaveBeenCalled()
  })

  it("awaits clicked-pane paste and propagates PTY write failure", async () => {
    seedTarget("unrelated-active-pane")
    const view = registerView({
      paste: vi.fn(async () => {
        throw new Error("pty write failed")
      }),
    })
    mockIPC((cmd) => cmd === "plugin:clipboard-manager|read_text" ? "clipboard" : undefined)

    await expect(pasteTerminalClipboard(target)).rejects.toThrow("pty write failed")
    expect(view.paste).toHaveBeenCalledExactlyOnceWith("clipboard")
  })

  it("does not copy when the clicked terminal has no selection", async () => {
    seedTarget()
    registerView({ hasSelection: vi.fn(() => false) })
    const calls: string[] = []
    mockIPC((cmd) => {
      calls.push(cmd)
      return undefined
    })

    expect(await copyTerminalSelection(target)).toBe("cancelled")
    expect(calls).not.toContain("plugin:clipboard-manager|write_text")
  })

  it("splits from the clicked pane even when no active-pane fallback is available", () => {
    seedTarget("missing-active-pane")

    expect(splitTerminal(target, "down")).toBe("completed")

    const layout = useTerminalStore.getState().layouts[target.workspacePath]
    expect(layout.panes).toHaveLength(2)
    expect(layout.panes[0]).toEqual({ paneId: target.paneId, sessionId: target.sessionId })
    expect(layout.splitDirection).toBe("down")
    expect(layout.activePaneId).toBe(layout.panes[1]?.paneId)
  })

  it("does not split a stale target or exceed the pane cap", () => {
    seedTarget()
    expect(splitTerminal({ ...target, paneId: "stale-pane" }, "right")).toBe("cancelled")

    useTerminalStore.getState().splitFrom(
      target.workspacePath,
      target.paneId,
      {
        sessionId: "session-2",
        title: "Terminal 2",
        workspace: target.workspacePath,
        shell: "",
        cols: 80,
        rows: 24,
      },
      "right"
    )
    expect(splitTerminal(target, "right")).toBe("cancelled")
    expect(useTerminalStore.getState().layouts[target.workspacePath].panes).toHaveLength(2)
  })

  it("cancel leaves pane state untouched and never closes the PTY", async () => {
    seedTarget()
    const calls: string[] = []
    mockIPC((cmd) => {
      calls.push(cmd)
      if (cmd === "plugin:dialog|message") return "Cancel"
      return undefined
    })

    expect(await closeTerminal(target)).toBe("cancelled")
    expect(calls).not.toContain("pty_close")
    expect(useTerminalStore.getState().layouts[target.workspacePath].panes).toHaveLength(1)
  })

  it("awaits PTY close before removing the clicked pane", async () => {
    seedTarget()
    const calls: string[] = []
    mockIPC((cmd, args) => {
      calls.push(cmd)
      if (cmd === "plugin:dialog|message") {
        expect(args).toMatchObject({
          title: "Close terminal",
          message: "Close terminal? The shell and its child processes will be terminated.",
        })
        return "Ok"
      }
      if (cmd === "pty_close") {
        expect(useTerminalStore.getState().layouts[target.workspacePath].panes).toHaveLength(1)
      }
      return undefined
    })

    expect(await closeTerminal(target)).toBe("completed")
    expect(calls).toEqual(["plugin:dialog|message", "pty_close"])
    expect(useTerminalStore.getState().layouts[target.workspacePath].panes).toEqual([])
  })

  it("preserves the pane when PTY close fails", async () => {
    seedTarget()
    mockIPC((cmd) => {
      if (cmd === "plugin:dialog|message") return "Ok"
      if (cmd === "pty_close") throw new Error("close failed")
      return undefined
    })

    await expect(closeTerminal(target)).rejects.toThrow("close failed")
    expect(useTerminalStore.getState().layouts[target.workspacePath].panes).toHaveLength(1)
    expect(useTerminalStore.getState().sessions[target.sessionId]).toBeDefined()
  })
})

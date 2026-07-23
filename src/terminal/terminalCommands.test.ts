import { beforeEach, describe, expect, it, vi } from "vitest"
import { clearMocks, mockIPC } from "@tauri-apps/api/mocks"

import { terminalDisplayTitle, useTerminalStore } from "@/state/terminalStore"
import {
  beginRenameTerminal,
  closeTerminal,
  createTerminalSessionMeta,
  splitTerminal,
  terminalPaneTargetExists,
  terminalTargetExists,
  type TerminalCommandTarget,
} from "./terminalCommands"

const terminalSettingsMock = vi.hoisted(() => ({
  value: {
    shellPath: "",
    shellArgs: "",
    defaultProfile: {
      id: "system",
      name: "System default",
      shell: "",
      args: [] as string[],
      kind: "system",
      cwdStrategy: "native",
    },
    imeAnchorMode: "cursor" as "cursor" | "tui",
  },
}))

vi.mock("@/app/workbench/settingsStorage", () => ({
  loadTerminalSettings: () => terminalSettingsMock.value,
}))

const target: TerminalCommandTarget = {
  workspacePath: "/workspace",
  paneId: "pane-clicked",
  sessionId: "session-clicked",
}

function seedTarget(activePaneId: string | null = target.paneId ?? null): void {
  useTerminalStore.setState({
    sessions: {
      [target.sessionId]: {
        sessionId: target.sessionId,
        title: "Terminal 1",
        launchStatus: "running",
        workspace: target.workspacePath,
        shell: "",
        cols: 80,
        rows: 24,
      },
    },
    layouts: {
      [target.workspacePath]: {
        tabIds: [target.sessionId],
        panes: [{ paneId: target.paneId!, sessionId: target.sessionId }],
        activePaneId,
        splitRatio: 0.5,
        nextTerminalNumber: 2,
        renamingSessionId: null,
      },
    },
  })
}

beforeEach(() => {
  clearMocks()
  useTerminalStore.getState().reset()
  terminalSettingsMock.value = {
    shellPath: "",
    shellArgs: "",
    defaultProfile: {
      id: "system",
      name: "System default",
      shell: "",
      args: [],
      kind: "system",
      cwdStrategy: "native",
    },
    imeAnchorMode: "cursor",
  }
})

describe("terminalCommands", () => {
  it("recognizes hidden tabs while requiring a visible pane for pane-only operations", () => {
    seedTarget()
    useTerminalStore.getState().addSession(target.workspacePath, {
      sessionId: "hidden",
      title: "Terminal 2",
      launchStatus: "running",
      workspace: target.workspacePath,
      shell: "",
      cols: 80,
      rows: 24,
    })

    const hidden = { ...target, paneId: undefined, sessionId: target.sessionId }
    expect(terminalTargetExists(hidden)).toBe(true)
    expect(terminalPaneTargetExists(hidden)).toBe(false)
  })

  it("allocates monotonic default names instead of reusing closed tab positions", () => {
    const first = createTerminalSessionMeta(target.workspacePath)
    useTerminalStore.getState().addSession(target.workspacePath, first)
    useTerminalStore.getState().removeSession(target.workspacePath, first.sessionId)
    const second = createTerminalSessionMeta(target.workspacePath)

    expect(first.title).toBe("Terminal 1")
    expect(second.title).toBe("Terminal 2")
    expect(second.launchStatus).toBe("opening")
  })

  it("snapshots the selected profile argv and IME anchor into a new session", () => {
    terminalSettingsMock.value = {
      shellPath: "legacy.exe",
      shellArgs: "--legacy",
      defaultProfile: {
        id: "powershell-7",
        name: "PowerShell 7",
        shell: "pwsh.exe",
        args: ["-NoExit", "-Command", "Write-Output 'hello world'"],
        kind: "powershell",
        cwdStrategy: "native",
      },
      imeAnchorMode: "tui",
    }

    expect(createTerminalSessionMeta(target.workspacePath)).toMatchObject({
      shell: "pwsh.exe",
      shellArgs: ["-NoExit", "-Command", "Write-Output 'hello world'"],
      imeAnchorMode: "tui",
      profileName: "PowerShell 7",
    })
  })

  it("allows New Terminal to override the default profile without changing settings", () => {
    expect(
      createTerminalSessionMeta(target.workspacePath, {
        id: "wsl:Ubuntu",
        name: "WSL: Ubuntu",
        shell: "wsl.exe",
        args: ["--distribution", "Ubuntu"],
        kind: "wsl",
        cwdStrategy: "wsl",
      }),
    ).toMatchObject({
      shell: "wsl.exe",
      shellArgs: ["--distribution", "Ubuntu"],
      cwdStrategy: "wsl",
      imeAnchorMode: "cursor",
      profileName: "WSL: Ubuntu",
    })
  })

  it("splits only from a visible pane and always appends a focused right tab", () => {
    seedTarget("missing-active-pane")

    expect(splitTerminal(target)).toBe("completed")

    const layout = useTerminalStore.getState().layouts[target.workspacePath]
    expect(layout.panes).toHaveLength(2)
    expect(layout.panes[0]).toEqual({ paneId: target.paneId, sessionId: target.sessionId })
    expect(layout.tabIds[1]).toBe(layout.panes[1]?.sessionId)
    expect(layout.activePaneId).toBe(layout.panes[1]?.paneId)
    expect(splitTerminal(target)).toBe("cancelled")
  })

  it("begins inline rename for a tab target", () => {
    seedTarget()
    const tabTarget = { ...target, paneId: undefined }

    expect(beginRenameTerminal(tabTarget)).toBe("completed")
    expect(useTerminalStore.getState().layouts[target.workspacePath].renamingSessionId)
      .toBe(target.sessionId)
  })

  it("closes an idle PTY directly without opening confirmation", async () => {
    seedTarget()
    const calls: string[] = []
    mockIPC((cmd) => {
      calls.push(cmd)
      if (cmd === "pty_activity") return "idle"
      return undefined
    })

    expect(await closeTerminal(target)).toBe("completed")
    expect(calls).toEqual(["pty_activity", "pty_close"])
    expect(useTerminalStore.getState().layouts[target.workspacePath].tabIds).toEqual([])
  })

  it.each(["busy", "unknown"] as const)(
    "prompts once before closing a %s PTY and includes its display title",
    async (activity) => {
      seedTarget()
      useTerminalStore.getState().setShellTitle(target.sessionId, "dev server")
      const calls: string[] = []
      mockIPC((cmd, args) => {
        calls.push(cmd)
        if (cmd === "pty_activity") return activity
        if (cmd === "plugin:dialog|message") {
          expect(args).toMatchObject({
            title: "Close terminal",
            message: expect.stringContaining("dev server"),
          })
          return "Ok"
        }
        return undefined
      })

      expect(await closeTerminal(target)).toBe("completed")
      expect(calls).toEqual(["pty_activity", "plugin:dialog|message", "pty_close"])
    }
  )

  it("leaves a busy tab untouched when confirmation is cancelled", async () => {
    seedTarget()
    mockIPC((cmd) => {
      if (cmd === "pty_activity") return "busy"
      if (cmd === "plugin:dialog|message") return "Cancel"
      return undefined
    })

    expect(await closeTerminal(target)).toBe("cancelled")
    expect(useTerminalStore.getState().sessions[target.sessionId]).toBeDefined()
  })

  it("treats activity query failure as Unknown instead of closing without protection", async () => {
    seedTarget()
    const calls: string[] = []
    mockIPC((cmd) => {
      calls.push(cmd)
      if (cmd === "pty_activity") throw new Error("unsupported")
      if (cmd === "plugin:dialog|message") return "Cancel"
      return undefined
    })

    expect(await closeTerminal(target)).toBe("cancelled")
    expect(calls).toEqual(["pty_activity", "plugin:dialog|message"])
  })

  it("closes a failed spawn without activity lookup or confirmation", async () => {
    seedTarget()
    useTerminalStore.getState().setLaunchStatus(target.sessionId, "failed")
    const calls: string[] = []
    mockIPC((cmd) => {
      calls.push(cmd)
      return undefined
    })

    expect(await closeTerminal({ ...target, paneId: undefined })).toBe("completed")
    expect(calls).toEqual(["pty_close"])
  })

  it("preserves the tab when PTY close fails", async () => {
    seedTarget()
    mockIPC((cmd) => {
      if (cmd === "pty_activity") return "idle"
      if (cmd === "pty_close") throw new Error("close failed")
      return undefined
    })

    await expect(closeTerminal(target)).rejects.toThrow("close failed")
    const session = useTerminalStore.getState().sessions[target.sessionId]
    expect(session).toBeDefined()
    expect(terminalDisplayTitle(session)).toBe("Terminal 1")
  })
})

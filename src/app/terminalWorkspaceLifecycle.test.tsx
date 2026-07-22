import { act, fireEvent, render, screen, waitFor } from "@testing-library/react"
import { clearMocks, mockIPC, mockWindows } from "@tauri-apps/api/mocks"
import { beforeEach, expect, it, vi } from "vitest"

import App from "@/App"
import type { PtyEvent } from "@/lib/types"
import { useRecentWorkspacesStore } from "@/state/recentWorkspaces"
import { terminalInitialState, useTerminalStore } from "@/state/terminalStore"
import { uiInitialState, useUiStore } from "@/state/uiStore"
import { useWorkspaceStore } from "@/state/workspaceStore"

const terminalMocks = vi.hoisted(() => {
  const state = {
    terminals: [] as TerminalMock[],
    eventHandlers: new Map<string, (event: PtyEvent) => void>(),
  }

  class TerminalMock {
    options: Record<string, unknown> = {}
    cols = 80
    rows = 24
    loadAddon = vi.fn((addon: { activate?: (terminal: TerminalMock) => void }) => {
      addon.activate?.(this)
    })
    open = vi.fn()
    onData = vi.fn(() => ({ dispose: vi.fn() }))
    onTitleChange = vi.fn(() => ({ dispose: vi.fn() }))
    attachCustomKeyEventHandler = vi.fn()
    write = vi.fn()
    focus = vi.fn()
    hasSelection = vi.fn(() => false)
    getSelection = vi.fn(() => "")
    paste = vi.fn()
    clear = vi.fn()
    dispose = vi.fn()

    constructor(options: Record<string, unknown>) {
      this.options = options
      state.terminals.push(this)
    }
  }

  class FitAddonMock {
    activate = vi.fn()
    fit = vi.fn()
    dispose = vi.fn()
  }

  return {
    state,
    TerminalMock,
    FitAddonMock,
    ptyOpen: vi.fn(),
    ptyWrite: vi.fn(),
    ptyResize: vi.fn(),
    ptyClose: vi.fn(),
    ptyCloseWorkspace: vi.fn(),
  }
})

const windowMocks = vi.hoisted(() => ({
  setTheme: vi.fn(() => Promise.resolve()),
  show: vi.fn(() => Promise.resolve()),
  onCloseRequested: vi.fn(() => Promise.resolve(() => {})),
}))

vi.mock("@xterm/xterm", () => ({
  Terminal: terminalMocks.TerminalMock,
}))

vi.mock("@xterm/addon-fit", () => ({
  FitAddon: terminalMocks.FitAddonMock,
}))

vi.mock("@/lib/ipc", async (importOriginal) => {
  const actual = await importOriginal<typeof import("@/lib/ipc")>()
  return {
    ...actual,
    ptyOpen: terminalMocks.ptyOpen,
    ptyWrite: terminalMocks.ptyWrite,
    ptyResize: terminalMocks.ptyResize,
    ptyClose: terminalMocks.ptyClose,
    ptyCloseWorkspace: terminalMocks.ptyCloseWorkspace,
  }
})

vi.mock("@tauri-apps/api/window", () => ({
  getCurrentWindow: () => windowMocks,
}))

vi.mock("@/features/logs/userAction", () => ({
  logUserAction: vi.fn(async () => undefined),
}))

function installLocalStorage(): void {
  const values = new Map<string, string>()
  Object.defineProperty(globalThis, "localStorage", {
    value: {
      getItem: (key: string) => values.get(key) ?? null,
      setItem: (key: string, value: string) => void values.set(key, String(value)),
      removeItem: (key: string) => void values.delete(key),
      clear: () => values.clear(),
      key: (index: number) => [...values.keys()][index] ?? null,
      get length() {
        return values.size
      },
    },
    configurable: true,
    writable: true,
  })
}

beforeEach(() => {
  clearMocks()
  vi.clearAllMocks()
  installLocalStorage()
  mockWindows("main")
  mockIPC((command, payload) => {
    if (command === "open_workspace") return (payload as { path: string }).path
    if (command === "list_dir") return []
    if (command === "agent_list") return []
    if (command === "plugin:dialog|message") return "Ok"
    return null
  })

  terminalMocks.ptyOpen.mockImplementation(
    async (
      workspace: string,
      sessionId: string,
      shell: string | null,
      _shellArgs: string[] | undefined,
      cols: number,
      rows: number,
      onEvent: (event: PtyEvent) => void
    ) => {
      terminalMocks.state.eventHandlers.set(sessionId, onEvent)
      return { workspace, sessionId, shell: shell ?? "/bin/zsh", cols, rows }
    }
  )
  terminalMocks.ptyWrite.mockResolvedValue(undefined)
  terminalMocks.ptyResize.mockResolvedValue(undefined)
  terminalMocks.ptyClose.mockResolvedValue(undefined)
  terminalMocks.ptyCloseWorkspace.mockResolvedValue(undefined)
  terminalMocks.state.terminals.length = 0
  terminalMocks.state.eventHandlers.clear()

  useTerminalStore.setState(terminalInitialState)
  useRecentWorkspacesStore.setState({ list: [] })
  useUiStore.setState({ ...uiInitialState, terminalOpen: true })
  useWorkspaceStore.setState({
    workspacePath: null,
    groups: [{ tabs: [], activePath: null }],
    activeGroupIndex: 0,
    pendingReveal: null,
  })
})

it("keeps a live terminal session mounted across an A to B to A workspace switch", async () => {
  const firstWorkspace = "/workspace-a"
  const secondWorkspace = "/workspace-b"
  const firstSession = {
    sessionId: "pty-a",
    title: "Terminal A",
    launchStatus: "running" as const,
    workspace: firstWorkspace,
    shell: "/bin/zsh",
    cols: 80,
    rows: 24,
  }

  useWorkspaceStore.setState({ workspacePath: firstWorkspace })
  useRecentWorkspacesStore.setState({ list: [firstWorkspace, secondWorkspace] })
  useTerminalStore.getState().addSession(firstWorkspace, firstSession)

  render(<App />)

  await waitFor(() => {
    expect(terminalMocks.ptyOpen).toHaveBeenCalledWith(
      firstWorkspace,
      firstSession.sessionId,
      firstSession.shell,
      undefined,
      80,
      24,
      expect.any(Function)
    )
  })
  const firstTerminal = screen.getByTestId(`terminal-session-${firstSession.sessionId}`)
  const firstXterm = terminalMocks.state.terminals[0]
  expect(firstTerminal).toBeVisible()

  fireEvent.click(screen.getByRole("button", { name: "Open workspace-b" }))

  await waitFor(() => {
    expect(useWorkspaceStore.getState().workspacePath).toBe(secondWorkspace)
    expect(useTerminalStore.getState().sessionsForWorkspace(firstWorkspace)).toEqual([
      firstSession,
    ])
  })
  expect(firstTerminal).toBeInTheDocument()
  expect(firstTerminal).not.toBeVisible()
  expect(firstTerminal.parentElement?.parentElement).toHaveAttribute("hidden")
  expect(firstTerminal.parentElement?.parentElement).toHaveAttribute("inert")

  act(() => {
    terminalMocks.state.eventHandlers.get(firstSession.sessionId)?.({
      type: "output",
      data: "background output\n",
    })
  })
  expect(firstXterm.write).toHaveBeenCalledWith("background output\n")

  await new Promise((resolve) => window.setTimeout(resolve, 0))
  expect(terminalMocks.ptyCloseWorkspace).not.toHaveBeenCalled()
  expect(terminalMocks.ptyClose).not.toHaveBeenCalledWith(firstSession.sessionId)

  const secondSession = {
    sessionId: "pty-b",
    title: "Terminal B",
    launchStatus: "running" as const,
    workspace: secondWorkspace,
    shell: "/bin/zsh",
    cols: 80,
    rows: 24,
  }
  act(() => useTerminalStore.getState().addSession(secondWorkspace, secondSession))
  const secondTerminal = await screen.findByTestId(
    `terminal-session-${secondSession.sessionId}`
  )
  expect(secondTerminal).toBeVisible()

  fireEvent.click(screen.getByRole("button", { name: "Open workspace-a" }))

  await waitFor(() => {
    expect(useWorkspaceStore.getState().workspacePath).toBe(firstWorkspace)
  })

  expect(screen.getByTestId(`terminal-session-${firstSession.sessionId}`)).toBe(firstTerminal)
  expect(firstTerminal).toBeVisible()
  expect(firstTerminal.parentElement?.parentElement).not.toHaveAttribute("hidden")
  expect(secondTerminal).not.toBeVisible()
  expect(useTerminalStore.getState().sessionsForWorkspace(secondWorkspace)).toEqual([
    secondSession,
  ])
  expect(
    terminalMocks.ptyOpen.mock.calls.filter((call) => call[1] === firstSession.sessionId)
  ).toHaveLength(1)
  expect(terminalMocks.state.terminals).toHaveLength(2)
  expect(terminalMocks.ptyCloseWorkspace).not.toHaveBeenCalled()
})

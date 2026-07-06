import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"
import { fireEvent, render, screen } from "@testing-library/react"

import { TerminalDrawer } from "@/app/workbench/TerminalDrawer"
import { useContextMenuStore } from "@/state/contextMenuStore"
import { useTerminalStore } from "@/state/terminalStore"
import { useWorkspaceStore } from "@/state/workspaceStore"

vi.mock("@/terminal/TerminalSession", () => ({
  TerminalSession: ({
    sessionId,
    shell,
    shellArgs,
    active,
  }: {
    sessionId: string
    shell: string | null
    shellArgs?: string[]
    active: boolean
  }) => (
    <button
      type="button"
      data-testid={`terminal-session-${sessionId}`}
      data-shell={shell ?? ""}
      data-shell-args={shellArgs?.join("|") ?? ""}
      data-active={String(active)}
    >
      Session {sessionId}
    </button>
  ),
}))

vi.mock("@/features/logs/userAction", () => ({
  logUserAction: vi.fn(async () => undefined),
}))

function installLocalStorage(): void {
  const store = new Map<string, string>()
  const mock = {
    getItem: (k: string) => (store.has(k) ? store.get(k)! : null),
    setItem: (k: string, v: string) => void store.set(k, String(v)),
    removeItem: (k: string) => void store.delete(k),
    clear: () => store.clear(),
    key: (i: number) => [...store.keys()][i] ?? null,
    get length() {
      return store.size
    },
  }
  Object.defineProperty(globalThis, "localStorage", {
    value: mock,
    configurable: true,
    writable: true,
  })
}

beforeEach(() => {
  installLocalStorage()
  useTerminalStore.getState().reset()
})

afterEach(() => {
  useContextMenuStore.setState({ kind: null, x: 0, y: 0, payload: {} })
  useWorkspaceStore.setState({ workspacePath: null })
})

describe("TerminalDrawer content resize", () => {
  it("tracks the pointer 1:1 while dragging (transition suppressed) and re-enables it on release", () => {
    render(<TerminalDrawer visible={true} />)

    const handle = screen.getByTitle("Drag to resize")
    const content = document.querySelector(".yzs.overflow-y-auto.font-mono") as HTMLElement
    expect(content.style.height).toBe("228px")
    expect(content.style.transition).toContain("height 220ms")

    fireEvent.pointerDown(handle, { clientY: 300, pointerId: 1 })
    expect(content.style.transition).toBe("none")

    fireEvent.pointerMove(handle, { clientY: 250, pointerId: 1 })
    expect(content.style.height).toBe("278px")

    fireEvent.pointerMove(handle, { clientY: 200, pointerId: 1 })
    expect(content.style.height).toBe("328px")

    fireEvent.pointerUp(handle, { clientY: 200, pointerId: 1 })
    expect(content.style.transition).toContain("height 220ms")
  })

  it("clamps the dragged height to the configured min/max", () => {
    const origH = window.innerHeight
    window.innerHeight = 1000 // tall enough that MAX_HEIGHT (480) is the binding cap
    try {
      render(<TerminalDrawer visible={true} />)

      const handle = screen.getByTitle("Drag to resize")
      const content = document.querySelector(".yzs.overflow-y-auto.font-mono") as HTMLElement

      fireEvent.pointerDown(handle, { clientY: 0, pointerId: 1 })
      fireEvent.pointerMove(handle, { clientY: 2000, pointerId: 1 })
      expect(content.style.height).toBe("140px")

      fireEvent.pointerMove(handle, { clientY: -2000, pointerId: 1 })
      expect(content.style.height).toBe("480px")

      fireEvent.pointerUp(handle, { clientY: -2000, pointerId: 1 })
    } finally {
      window.innerHeight = origH
    }
  })

  it("caps the dragged height at 60% of a short window", () => {
    const origH = window.innerHeight
    window.innerHeight = 600 // 60% -> 360px cap, below MAX_HEIGHT
    try {
      render(<TerminalDrawer visible={true} />)

      const handle = screen.getByTitle("Drag to resize")
      const content = document.querySelector(".yzs.overflow-y-auto.font-mono") as HTMLElement

      fireEvent.pointerDown(handle, { clientY: 0, pointerId: 1 })
      fireEvent.pointerMove(handle, { clientY: -2000, pointerId: 1 })
      expect(content.style.height).toBe("360px")

      fireEvent.pointerUp(handle, { clientY: -2000, pointerId: 1 })
    } finally {
      window.innerHeight = origH
    }
  })
})

it("右鍵 terminal drawer 開啟 terminal 選單", () => {
  render(<TerminalDrawer visible />)
  fireEvent.contextMenu(screen.getByText("Terminal"))
  expect(useContextMenuStore.getState().kind).toBe("terminal")
})

describe("TerminalDrawer sessions", () => {
  it("does not create sessions from render or visibility alone", () => {
    useWorkspaceStore.setState({ workspacePath: "/workspace" })

    render(<TerminalDrawer visible={true} />)

    expect(useTerminalStore.getState().sessionsForWorkspace("/workspace")).toHaveLength(0)
    expect(screen.getByText("尚無終端機工作階段")).toBeInTheDocument()
    expect(screen.queryAllByTestId(/terminal-session-/)).toHaveLength(0)
  })

  it("opens a new terminal session from the New action", () => {
    useWorkspaceStore.setState({ workspacePath: "/workspace" })
    render(<TerminalDrawer visible={true} />)

    fireEvent.click(screen.getByTitle("New terminal"))

    const sessions = screen.getAllByTestId(/terminal-session-/)
    expect(sessions).toHaveLength(1)
    expect(sessions[0]).toHaveAttribute("data-active", "true")
    expect(screen.queryByText("尚無終端機工作階段")).not.toBeInTheDocument()
    expect(useTerminalStore.getState().sessionsForWorkspace("/workspace")).toHaveLength(1)
  })

  it("passes the persisted shell override to new terminal sessions", () => {
    localStorage.setItem(
      "yuzora:terminal-settings",
      JSON.stringify({ shellPath: "/opt/homebrew/bin/fish", shellArgs: "" })
    )
    useWorkspaceStore.setState({ workspacePath: "/workspace" })
    render(<TerminalDrawer visible={true} />)

    fireEvent.click(screen.getByTitle("New terminal"))

    expect(screen.getByTestId(/terminal-session-/)).toHaveAttribute(
      "data-shell",
      "/opt/homebrew/bin/fish"
    )
    expect(useTerminalStore.getState().sessionsForWorkspace("/workspace")[0]?.shell).toBe(
      "/opt/homebrew/bin/fish"
    )
  })

  it("passes persisted shell args to new terminal sessions", () => {
    localStorage.setItem(
      "yuzora:terminal-settings",
      JSON.stringify({ shellPath: "/bin/sh", shellArgs: "-c echo-ok" })
    )
    useWorkspaceStore.setState({ workspacePath: "/workspace" })
    render(<TerminalDrawer visible={true} />)

    fireEvent.click(screen.getByTitle("New terminal"))

    expect(screen.getByTestId(/terminal-session-/)).toHaveAttribute("data-shell-args", "-c|echo-ok")
    expect(useTerminalStore.getState().sessionsForWorkspace("/workspace")[0]).toMatchObject({
      shellArgs: ["-c", "echo-ok"],
    })
  })

  it("closes the active session and returns to the empty state", () => {
    useWorkspaceStore.setState({ workspacePath: "/workspace" })
    render(<TerminalDrawer visible={true} />)

    fireEvent.click(screen.getByTitle("New terminal"))
    fireEvent.click(screen.getByTitle("Close terminal"))

    expect(screen.queryAllByTestId(/terminal-session-/)).toHaveLength(0)
    expect(screen.getByText("尚無終端機工作階段")).toBeInTheDocument()
    expect(useTerminalStore.getState().sessionsForWorkspace("/workspace")).toHaveLength(0)
  })

  it("switches the active pane when a pane is clicked", () => {
    useWorkspaceStore.setState({ workspacePath: "/workspace" })
    render(<TerminalDrawer visible={true} />)

    fireEvent.click(screen.getByTitle("New terminal"))
    fireEvent.click(screen.getByTitle("Split right"))

    const sessions = screen.getAllByTestId(/terminal-session-/)
    expect(sessions).toHaveLength(2)
    expect(sessions[0]).toHaveAttribute("data-active", "false")
    expect(sessions[1]).toHaveAttribute("data-active", "true")

    fireEvent.click(sessions[0])
    expect(screen.getAllByTestId(/terminal-session-/)[0]).toHaveAttribute("data-active", "true")
    expect(screen.getAllByTestId(/terminal-session-/)[1]).toHaveAttribute("data-active", "false")
  })

  it("splits once and disables split/new actions at the two-pane cap", () => {
    useWorkspaceStore.setState({ workspacePath: "/workspace" })
    render(<TerminalDrawer visible={true} />)

    fireEvent.click(screen.getByTitle("New terminal"))
    fireEvent.click(screen.getByTitle("Split down"))

    expect(screen.getAllByTestId(/terminal-session-/)).toHaveLength(2)
    expect(screen.getByTitle("Split right")).toBeDisabled()
    expect(screen.getByTitle("Split down")).toBeDisabled()
    expect(screen.getByTitle("New terminal")).toBeDisabled()

    fireEvent.click(screen.getByTitle("Split down"))
    expect(screen.getAllByTestId(/terminal-session-/)).toHaveLength(2)
  })
})

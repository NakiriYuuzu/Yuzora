import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"
import { createRef } from "react"
import { act, fireEvent, render, screen, waitFor } from "@testing-library/react"
import { clearMocks, mockIPC } from "@tauri-apps/api/mocks"

import { TerminalDrawer } from "@/app/workbench/TerminalDrawer"
import i18n from "@/lib/i18n"
import { useContextMenuStore } from "@/state/contextMenuStore"
import { useTerminalStore } from "@/state/terminalStore"
import {
  useWorkbenchLayoutStore,
  workbenchLayoutInitialState,
} from "@/state/workbenchLayoutStore"
import { useWorkspaceStore } from "@/state/workspaceStore"

vi.mock("@/terminal/TerminalSession", () => ({
  TerminalSession: ({
    sessionId,
    workspace,
    shell,
    shellArgs,
    active,
  }: {
    sessionId: string
    workspace: string
    shell: string | null
    shellArgs?: string[]
    active: boolean
  }) => (
    <button
      type="button"
      data-testid={`terminal-session-${sessionId}`}
      data-workspace={workspace}
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

interface ResizeObservation {
  callback: ResizeObserverCallback
  observer: ResizeObserver
  target: Element | null
}

const resizeObservations: ResizeObservation[] = []
const defaultResizeObserver = globalThis.ResizeObserver

class ResizeObserverHarness {
  private readonly observation: ResizeObservation

  constructor(callback: ResizeObserverCallback) {
    this.observation = {
      callback,
      observer: this as unknown as ResizeObserver,
      target: null,
    }
    resizeObservations.push(this.observation)
  }

  observe(target: Element) {
    this.observation.target = target
  }

  unobserve() {}
  disconnect() {}
}

function rect(height: number, top = 100): DOMRect {
  return {
    x: 0,
    y: top,
    width: 800,
    height,
    top,
    right: 800,
    bottom: top + height,
    left: 0,
    toJSON: () => ({}),
  } as DOMRect
}

function resizeContainer(container: HTMLElement, height: number, top = 100): void {
  const nextRect = rect(height, top)
  vi.spyOn(container, "getBoundingClientRect").mockReturnValue(nextRect)
  const observation = resizeObservations.find((candidate) => candidate.target === container)
  if (!observation) throw new Error("TerminalDrawer did not observe its workspace stack")

  act(() => {
    observation.callback(
      [{ target: container, contentRect: nextRect } as unknown as ResizeObserverEntry],
      observation.observer
    )
  })
}

function renderDrawer({
  visible = true,
  height = 800,
  mainSurfaceMinHeight = 44,
}: {
  visible?: boolean
  height?: number
  mainSurfaceMinHeight?: number
} = {}) {
  const containerRef = createRef<HTMLDivElement>()
  const view = render(
    <div ref={containerRef} data-testid="terminal-test-stack">
      <TerminalDrawer
        visible={visible}
        containerRef={containerRef}
        mainSurfaceMinHeight={mainSurfaceMinHeight}
      />
    </div>
  )
  const container = screen.getByTestId("terminal-test-stack")
  resizeContainer(container, height)
  return { ...view, container, containerRef }
}

beforeEach(() => {
  clearMocks()
  mockIPC((cmd) => cmd === "plugin:dialog|message" ? "Ok" : undefined)
  installLocalStorage()
  resizeObservations.length = 0
  globalThis.ResizeObserver = ResizeObserverHarness as unknown as typeof ResizeObserver
  useTerminalStore.getState().reset()
  useWorkbenchLayoutStore.setState({
    ...workbenchLayoutInitialState,
    terminalWorkspaceRatios: {},
  })
})

afterEach(() => {
  useContextMenuStore.setState({ request: null, x: 0, y: 0, availabilityRevision: 0 })
  useWorkspaceStore.setState({ workspacePath: null })
  globalThis.ResizeObserver = defaultResizeObserver
})

describe("TerminalDrawer content resize", () => {
  it("derives full drawer allocation from the stored ratio and commits only on pointer release", () => {
    renderDrawer()

    const handle = screen.getByRole("separator", { name: "Resize terminal" })
    const drawer = screen.getByTestId("terminal-drawer")
    const content = document.querySelector(".yzs.overflow-y-auto.font-mono") as HTMLElement
    expect(drawer.style.height).toBe("237px")
    expect(content.style.height).toBe("193px")
    expect(content.style.transition).toContain("height 220ms")
    expect(handle).toHaveAttribute("aria-orientation", "horizontal")
    expect(handle).toHaveAttribute("aria-valuenow", "30")
    expect(handle).toHaveAttribute("aria-valuetext", "Main 70% · Terminal 30%")

    fireEvent.pointerDown(handle, { button: 0, clientY: 663, pointerId: 1 })
    expect(content.style.transition).toBe("none")

    fireEvent.pointerMove(handle, { clientY: 505, pointerId: 1 })
    expect(drawer.style.height).toBe("395px")
    expect(content.style.height).toBe("351px")
    expect(screen.getByText("Main 50% · Terminal 50%")).toBeInTheDocument()
    expect(useWorkbenchLayoutStore.getState().terminalGlobalRatio).toBe(0.3)

    fireEvent.pointerUp(handle, { clientY: 505, pointerId: 1 })
    expect(content.style.transition).toContain("height 220ms")
    expect(screen.queryByText("Main 50% · Terminal 50%")).not.toBeInTheDocument()
    expect(useWorkbenchLayoutStore.getState().terminalGlobalRatio).toBe(0.5)
  })

  it("clamps between the 140px content minimum and the 44px main-surface floor without a legacy cap", () => {
    renderDrawer()

    const handle = screen.getByRole("separator", { name: "Resize terminal" })
    const drawer = screen.getByTestId("terminal-drawer")
    const content = document.querySelector(".yzs.overflow-y-auto.font-mono") as HTMLElement

    fireEvent.pointerDown(handle, { button: 0, clientY: 663, pointerId: 1 })
    fireEvent.pointerMove(handle, { clientY: 2000, pointerId: 1 })
    expect(drawer.style.height).toBe("184px")
    expect(content.style.height).toBe("140px")

    fireEvent.pointerMove(handle, { clientY: -2000, pointerId: 1 })
    expect(drawer.style.height).toBe("746px")
    expect(content.style.height).toBe("702px")
    expect(Number(drawer.style.height.replace("px", "")) + 44 + 10).toBe(800)
    fireEvent.pointerUp(handle, { clientY: -2000, pointerId: 1 })
  })

  it("lets the Terminal content shrink safely when an extremely short container must preserve the main floor", () => {
    const { container } = renderDrawer({ height: 180 })

    const drawer = screen.getByTestId("terminal-drawer")
    const content = document.querySelector(".yzs.overflow-y-auto.font-mono") as HTMLElement

    expect(drawer.style.height).toBe("126px")
    expect(content.style.height).toBe("82px")
    expect(Number(drawer.style.height.replace("px", "")) + 44 + 10).toBe(180)
    expect(useWorkbenchLayoutStore.getState().terminalGlobalRatio).toBe(0.3)

    resizeContainer(container, 800)
    expect(drawer.style.height).toBe("237px")
    expect(content.style.height).toBe("193px")
    expect(useWorkbenchLayoutStore.getState().terminalGlobalRatio).toBe(0.3)
  })

  it("keeps the stored preference when container clamping changes and restores it when space returns", () => {
    useWorkbenchLayoutStore.getState().setTerminalRatio(null, 0.95)
    const { container } = renderDrawer()
    const drawer = screen.getByTestId("terminal-drawer")

    expect(drawer.style.height).toBe("746px")
    expect(useWorkbenchLayoutStore.getState().terminalGlobalRatio).toBe(0.95)

    resizeContainer(container, 1200)
    expect(drawer.style.height).toBe("1130.5px")
    expect(useWorkbenchLayoutStore.getState().terminalGlobalRatio).toBe(0.95)
  })

  it("preserves a 0.95 preference across files-agent-files floors and gives Agent exactly 280px", async () => {
    useWorkbenchLayoutStore.getState().setTerminalRatio(null, 0.95)
    const { rerender, containerRef } = renderDrawer({
      height: 800,
      mainSurfaceMinHeight: 44,
    })
    const drawer = screen.getByTestId("terminal-drawer")
    const content = document.querySelector(".yzs.overflow-y-auto.font-mono") as HTMLElement

    expect(drawer.style.height).toBe("746px")

    rerender(
      <div ref={containerRef} data-testid="terminal-test-stack">
        <TerminalDrawer visible containerRef={containerRef} mainSurfaceMinHeight={280} />
      </div>
    )

    expect(drawer.style.height).toBe("510px")
    expect(content.style.height).toBe("466px")
    expect(Number(drawer.style.height.replace("px", "")) + 280 + 10).toBe(800)
    expect(drawer.style.transition).toBe("none")
    expect(useWorkbenchLayoutStore.getState().terminalGlobalRatio).toBe(0.95)

    await waitFor(() => expect(drawer.style.transition).toBe(""))
    rerender(
      <div ref={containerRef} data-testid="terminal-test-stack">
        <TerminalDrawer visible containerRef={containerRef} mainSurfaceMinHeight={44} />
      </div>
    )

    expect(drawer.style.height).toBe("746px")
    expect(drawer.style.transition).toBe("none")
    expect(useWorkbenchLayoutStore.getState().terminalGlobalRatio).toBe(0.95)
  })

  it("prioritizes the Agent floor in a short viewport and disables a zero-range separator", () => {
    useWorkbenchLayoutStore.getState().setTerminalRatio(null, 0.95)
    renderDrawer({ height: 240, mainSurfaceMinHeight: 280 })
    const drawer = screen.getByTestId("terminal-drawer")
    const content = document.querySelector(".yzs.overflow-y-auto.font-mono") as HTMLElement
    const handle = screen.getByRole("separator", { name: "Resize terminal" })

    expect(drawer.style.height).toBe("0px")
    expect(content.style.height).toBe("0px")
    expect(Number(drawer.style.height.replace("px", ""))).toBeGreaterThanOrEqual(0)
    expect(Number(content.style.height.replace("px", ""))).toBeGreaterThanOrEqual(0)
    expect(handle).toHaveAttribute("aria-disabled", "true")
    expect(handle).toHaveAttribute("tabindex", "-1")
    expect(handle).not.toHaveAttribute("aria-valuemin")
    expect(handle).not.toHaveAttribute("aria-valuemax")
    expect(handle).not.toHaveAttribute("aria-valuenow")
    expect(handle).not.toHaveAttribute("aria-valuetext")
    expect(handle).not.toHaveAttribute("title")
    expect(handle).toHaveClass("cursor-default")
    expect(handle).not.toHaveClass("cursor-row-resize")

    fireEvent.keyDown(handle, { key: "ArrowDown" })
    expect(useWorkbenchLayoutStore.getState().terminalGlobalRatio).toBe(0.95)
  })

  it("cancels an active drag on floor change without committing the transient ratio", () => {
    const { rerender, containerRef } = renderDrawer({
      height: 800,
      mainSurfaceMinHeight: 44,
    })
    const handle = screen.getByRole("separator", { name: "Resize terminal" })
    const drawer = screen.getByTestId("terminal-drawer")

    fireEvent.pointerDown(handle, { button: 0, clientY: 663, pointerId: 1 })
    fireEvent.pointerMove(handle, { clientY: 505, pointerId: 1 })
    expect(drawer.style.height).toBe("395px")
    expect(useWorkbenchLayoutStore.getState().terminalGlobalRatio).toBe(0.3)

    rerender(
      <div ref={containerRef} data-testid="terminal-test-stack">
        <TerminalDrawer visible containerRef={containerRef} mainSurfaceMinHeight={280} />
      </div>
    )

    expect(drawer.style.height).toBe("237px")
    expect(drawer.style.transition).toBe("none")
    expect(screen.queryByText("Main 50% · Terminal 50%")).not.toBeInTheDocument()
    fireEvent.pointerUp(handle, { clientY: 505, pointerId: 1 })
    expect(useWorkbenchLayoutStore.getState().terminalGlobalRatio).toBe(0.3)
  })

  it("applies a safer allocation immediately when the container shrinks", async () => {
    useWorkbenchLayoutStore.getState().setTerminalRatio(null, 0.95)
    const { container } = renderDrawer({ height: 1200 })
    const drawer = screen.getByTestId("terminal-drawer")

    expect(drawer.style.height).toBe("1130.5px")
    resizeContainer(container, 800)
    expect(drawer.style.height).toBe("746px")
    expect(drawer.style.transition).toBe("none")
    expect(Number(drawer.style.height.replace("px", "")) + 44 + 10).toBe(800)

    await waitFor(() => expect(drawer.style.transition).toBe(""))
    fireEvent.click(screen.getByRole("button", { name: "Collapse terminal" }))
    expect(drawer.style.height).toBe("38px")
    expect(drawer.style.transition).toBe("")
  })

  it("does not replace a geometry-clamped preference for an outward keyboard no-op", () => {
    useWorkbenchLayoutStore.getState().setTerminalRatio(null, 0.95)
    renderDrawer({ height: 800 })
    const handle = screen.getByRole("separator", { name: "Resize terminal" })

    expect(handle).toHaveAttribute("aria-valuenow", "94")
    fireEvent.keyDown(handle, { key: "ArrowUp" })
    expect(useWorkbenchLayoutStore.getState().terminalGlobalRatio).toBe(0.95)

    fireEvent.keyDown(handle, { key: "ArrowDown" })
    expect(useWorkbenchLayoutStore.getState().terminalGlobalRatio).toBeCloseTo(
      746 / 790 - 0.02
    )
  })

  it("does not replace a geometry-clamped preference for an outward pointer no-op", () => {
    useWorkbenchLayoutStore.getState().setTerminalRatio(null, 0.95)
    renderDrawer({ height: 800 })
    const handle = screen.getByRole("separator", { name: "Resize terminal" })

    fireEvent.pointerDown(handle, { button: 0, clientY: 154, pointerId: 1 })
    fireEvent.pointerMove(handle, { clientY: -2000, pointerId: 1 })
    fireEvent.pointerUp(handle, { clientY: -2000, pointerId: 1 })
    expect(useWorkbenchLayoutStore.getState().terminalGlobalRatio).toBe(0.95)

    fireEvent.pointerDown(handle, { button: 0, clientY: 154, pointerId: 2 })
    fireEvent.pointerMove(handle, { clientY: 189, pointerId: 2 })
    fireEvent.pointerUp(handle, { clientY: 189, pointerId: 2 })
    expect(useWorkbenchLayoutStore.getState().terminalGlobalRatio).toBeCloseTo(0.9)
  })

  it("commits ArrowUp/ArrowDown directly with 2% and Shift+10% steps", () => {
    renderDrawer()
    const handle = screen.getByRole("separator", { name: "Resize terminal" })

    fireEvent.keyDown(handle, { key: "ArrowUp" })
    expect(useWorkbenchLayoutStore.getState().terminalGlobalRatio).toBeCloseTo(0.32)
    expect(handle).toHaveAttribute("aria-valuetext", "Main 68% · Terminal 32%")

    fireEvent.keyDown(handle, { key: "ArrowDown" })
    expect(useWorkbenchLayoutStore.getState().terminalGlobalRatio).toBeCloseTo(0.3)

    fireEvent.keyDown(handle, { key: "ArrowUp", shiftKey: true })
    expect(useWorkbenchLayoutStore.getState().terminalGlobalRatio).toBeCloseTo(0.4)
  })

  it("commits the current workspace override on pointer cancel and lost capture", () => {
    useWorkspaceStore.setState({ workspacePath: "/workspace" })
    useWorkbenchLayoutStore.getState().setTerminalRatio(null, 0.4)
    useWorkbenchLayoutStore.getState().setTerminalRatioScope("workspace", "/workspace")
    renderDrawer()
    const handle = screen.getByRole("separator", { name: "Resize terminal" })

    fireEvent.pointerDown(handle, { button: 0, clientY: 584, pointerId: 1 })
    fireEvent.pointerMove(handle, { clientY: 505, pointerId: 1 })
    fireEvent.pointerCancel(handle, { pointerId: 1 })
    expect(useWorkbenchLayoutStore.getState().terminalWorkspaceRatios["/workspace"]).toBeCloseTo(0.5)
    expect(useWorkbenchLayoutStore.getState().terminalGlobalRatio).toBe(0.4)

    fireEvent.pointerDown(handle, { button: 0, clientY: 505, pointerId: 2 })
    fireEvent.pointerMove(handle, { clientY: 426, pointerId: 2 })
    fireEvent.lostPointerCapture(handle, { pointerId: 2 })
    expect(useWorkbenchLayoutStore.getState().terminalWorkspaceRatios["/workspace"]).toBeCloseTo(0.6)
    expect(screen.queryByText(/Main .* Terminal/)).not.toBeInTheDocument()
  })

  it("collapses to the 38px header and restores the effective ratio after expand and rail hide/show", () => {
    const containerRef = createRef<HTMLDivElement>()
    const view = render(
      <div ref={containerRef} data-testid="terminal-test-stack">
        <TerminalDrawer visible containerRef={containerRef} mainSurfaceMinHeight={44} />
      </div>
    )
    const container = screen.getByTestId("terminal-test-stack")
    resizeContainer(container, 800)
    const drawer = screen.getByTestId("terminal-drawer")
    const content = document.querySelector(".yzs.overflow-y-auto.font-mono") as HTMLElement

    expect(drawer.style.height).toBe("237px")
    fireEvent.click(screen.getByRole("button", { name: "Collapse terminal" }))
    expect(drawer.style.height).toBe("38px")
    expect(content.style.height).toBe("0px")
    expect(useWorkbenchLayoutStore.getState().terminalGlobalRatio).toBe(0.3)

    fireEvent.click(screen.getByRole("button", { name: "Expand terminal" }))
    expect(drawer.style.height).toBe("237px")
    view.rerender(
      <div ref={containerRef} data-testid="terminal-test-stack">
        <TerminalDrawer
          visible={false}
          containerRef={containerRef}
          mainSurfaceMinHeight={44}
        />
      </div>
    )
    expect(drawer.style.height).toBe("0px")
    view.rerender(
      <div ref={containerRef} data-testid="terminal-test-stack">
        <TerminalDrawer visible containerRef={containerRef} mainSurfaceMinHeight={44} />
      </div>
    )
    expect(drawer.style.height).toBe("237px")
    expect(useWorkbenchLayoutStore.getState().terminalGlobalRatio).toBe(0.3)
  })
})

it("header 與 empty state 不會開啟 terminal entity menu", () => {
  renderDrawer()
  fireEvent.contextMenu(screen.getByText("Terminal"))
  fireEvent.contextMenu(screen.getByText(i18n.t("noSessions", { ns: "terminal" })))
  expect(useContextMenuStore.getState().request).toBeNull()
})

describe("TerminalDrawer sessions", () => {
  it("sanitizes the visible workspace label while passing the raw path to the terminal", () => {
    const rawPath = "\\\\?\\C:\\Users\\Yuuzu\\專案 空間 #100%"
    useWorkspaceStore.setState({ workspacePath: rawPath })
    renderDrawer()

    expect(screen.getByText("C:\\Users\\Yuuzu\\專案 空間 #100%")).toBeInTheDocument()
    expect(screen.queryByText(rawPath)).toBeNull()

    fireEvent.click(screen.getByTitle("New terminal"))
    expect(screen.getByTestId(/terminal-session-/)).toHaveAttribute("data-workspace", rawPath)
  })

  it("opens new sessions in the current workspace without moving existing sessions", () => {
    const firstWorkspace = "\\\\?\\C:\\Users\\Yuuzu\\第一個 專案 #100%"
    const secondWorkspace = "\\\\?\\D:\\工作區\\第二個 專案 #50%"
    useWorkspaceStore.setState({ workspacePath: firstWorkspace })
    renderDrawer()

    fireEvent.click(screen.getByTitle("New terminal"))
    const firstSession = useTerminalStore.getState().sessionsForWorkspace(firstWorkspace)[0]
    expect(firstSession?.workspace).toBe(firstWorkspace)
    const firstTerminal = screen.getByTestId(`terminal-session-${firstSession.sessionId}`)
    expect(firstTerminal).toBeVisible()

    act(() => useWorkspaceStore.setState({ workspacePath: secondWorkspace }))
    expect(firstTerminal).toBeInTheDocument()
    expect(firstTerminal).not.toBeVisible()
    fireEvent.click(screen.getByTitle("New terminal"))

    const secondSession = useTerminalStore.getState().sessionsForWorkspace(secondWorkspace)[0]
    expect(secondSession?.workspace).toBe(secondWorkspace)
    const secondTerminal = screen.getByTestId(`terminal-session-${secondSession.sessionId}`)
    expect(secondTerminal).toHaveAttribute(
      "data-workspace",
      secondWorkspace
    )
    expect(secondTerminal).toBeVisible()
    expect(useTerminalStore.getState().sessionsForWorkspace(firstWorkspace)).toEqual([
      firstSession,
    ])

    act(() => useWorkspaceStore.setState({ workspacePath: firstWorkspace }))
    expect(screen.getByTestId(`terminal-session-${firstSession.sessionId}`)).toBe(firstTerminal)
    expect(firstTerminal).toHaveAttribute("data-workspace", firstWorkspace)
    expect(firstTerminal).toBeVisible()
    expect(secondTerminal).not.toBeVisible()
    expect(useTerminalStore.getState().sessionsForWorkspace(secondWorkspace)).toEqual([
      secondSession,
    ])
  })

  it("does not create sessions from render or visibility alone", () => {
    useWorkspaceStore.setState({ workspacePath: "/workspace" })

    renderDrawer()

    expect(useTerminalStore.getState().sessionsForWorkspace("/workspace")).toHaveLength(0)
    expect(screen.getByText(i18n.t("noSessions", { ns: "terminal" }))).toBeInTheDocument()
    expect(screen.queryAllByTestId(/terminal-session-/)).toHaveLength(0)
  })

  it("opens a new terminal session from the New action", () => {
    useWorkspaceStore.setState({ workspacePath: "/workspace" })
    renderDrawer()

    fireEvent.click(screen.getByTitle("New terminal"))

    const sessions = screen.getAllByTestId(/terminal-session-/)
    expect(sessions).toHaveLength(1)
    expect(sessions[0]).toHaveAttribute("data-active", "true")
    expect(screen.queryByText(i18n.t("noSessions", { ns: "terminal" }))).not.toBeInTheDocument()
    expect(useTerminalStore.getState().sessionsForWorkspace("/workspace")).toHaveLength(1)
  })

  it("opens a terminal menu only from the clicked pane with a stable target snapshot", () => {
    useWorkspaceStore.setState({ workspacePath: "/workspace" })
    renderDrawer()
    fireEvent.click(screen.getByTitle("New terminal"))

    const pane = useTerminalStore.getState().layouts["/workspace"].panes[0]
    fireEvent.contextMenu(screen.getByTestId(`terminal-pane-${pane.paneId}`))

    expect(useContextMenuStore.getState().request).toEqual({
      kind: "terminal",
      workspacePath: "/workspace",
      paneId: pane.paneId,
      sessionId: pane.sessionId,
    })
  })

  it("removes the fake Dock toolbar action", () => {
    renderDrawer()
    expect(screen.queryByTitle("Dock terminal into editor")).not.toBeInTheDocument()
  })

  it("passes the persisted shell override to new terminal sessions", () => {
    localStorage.setItem(
      "yuzora:terminal-settings",
      JSON.stringify({ shellPath: "/opt/homebrew/bin/fish", shellArgs: "" })
    )
    useWorkspaceStore.setState({ workspacePath: "/workspace" })
    renderDrawer()

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
    renderDrawer()

    fireEvent.click(screen.getByTitle("New terminal"))

    expect(screen.getByTestId(/terminal-session-/)).toHaveAttribute("data-shell-args", "-c|echo-ok")
    expect(useTerminalStore.getState().sessionsForWorkspace("/workspace")[0]).toMatchObject({
      shellArgs: ["-c", "echo-ok"],
    })
  })

  it("closes the active PTY through the shared command and returns to the empty state", async () => {
    useWorkspaceStore.setState({ workspacePath: "/workspace" })
    renderDrawer()

    fireEvent.click(screen.getByTitle("New terminal"))
    fireEvent.click(screen.getByTitle("Close terminal"))

    await waitFor(() => expect(screen.queryAllByTestId(/terminal-session-/)).toHaveLength(0))
    expect(screen.getByText(i18n.t("noSessions", { ns: "terminal" }))).toBeInTheDocument()
    expect(useTerminalStore.getState().sessionsForWorkspace("/workspace")).toHaveLength(0)
  })

  it("switches the active pane when a pane is clicked", () => {
    useWorkspaceStore.setState({ workspacePath: "/workspace" })
    renderDrawer()

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
    renderDrawer()

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

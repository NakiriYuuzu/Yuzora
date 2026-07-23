import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"
import { createRef } from "react"
import { act, fireEvent, render, screen, waitFor, within } from "@testing-library/react"
import { clearMocks, mockIPC } from "@tauri-apps/api/mocks"

import { TerminalDrawer } from "@/app/workbench/TerminalDrawer"
import i18n from "@/lib/i18n"
import { contextMenuHandler, useContextMenuStore } from "@/state/contextMenuStore"
import { useTerminalStore } from "@/state/terminalStore"
import { useWorkbenchLayoutStore, workbenchLayoutInitialState } from "@/state/workbenchLayoutStore"
import { useWorkspaceStore } from "@/state/workspaceStore"

interface TerminalSessionMockProps {
  sessionId: string
  workspace: string
  shell: string | null
  shellArgs?: string[]
  imeAnchorMode?: "cursor" | "tui"
  active: boolean
  visible?: boolean
  onExit?: (code: number | null) => void
  onTitleChange?: (title: string) => void
  onReady?: () => void
  onOpenError?: (message: string) => void
}

const terminalSessionMocks = vi.hoisted(() => ({
  props: new Map<string, TerminalSessionMockProps>(),
}))

vi.mock("@/terminal/TerminalSession", () => ({
  TerminalSession: (props: TerminalSessionMockProps) => {
    terminalSessionMocks.props.set(props.sessionId, props)
    return (
      <button
        type="button"
        data-testid={`terminal-session-${props.sessionId}`}
        data-workspace={props.workspace}
        data-shell={props.shell ?? ""}
        data-shell-args={props.shellArgs?.join("|") ?? ""}
        data-ime-anchor={props.imeAnchorMode ?? "cursor"}
        data-active={String(props.active)}
        data-visible={String(props.visible ?? true)}
      >
        Session {props.sessionId}
      </button>
    )
  },
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
      [
        {
          target: container,
          contentRect: nextRect,
        } as unknown as ResizeObserverEntry,
      ],
      observation.observer,
    )
  })
}

function renderDrawer({
  visible = true,
  height = 800,
  mainSurfaceMinHeight = 44,
  includeAppShellContextMenu = false,
}: {
  visible?: boolean
  height?: number
  mainSurfaceMinHeight?: number
  includeAppShellContextMenu?: boolean
} = {}) {
  const containerRef = createRef<HTMLDivElement>()
  const view = render(
    <div
      ref={containerRef}
      data-testid="terminal-test-stack"
      onContextMenu={
        includeAppShellContextMenu ? contextMenuHandler({ kind: "general" }) : undefined
      }
    >
      <TerminalDrawer
        visible={visible}
        containerRef={containerRef}
        mainSurfaceMinHeight={mainSurfaceMinHeight}
      />
    </div>,
  )
  const container = screen.getByTestId("terminal-test-stack")
  resizeContainer(container, height)
  return { ...view, container, containerRef }
}

beforeEach(() => {
  clearMocks()
  mockIPC((cmd) => (cmd === "plugin:dialog|message" ? "Ok" : undefined))
  installLocalStorage()
  resizeObservations.length = 0
  terminalSessionMocks.props.clear()
  globalThis.ResizeObserver = ResizeObserverHarness as unknown as typeof ResizeObserver
  useTerminalStore.getState().reset()
  useWorkbenchLayoutStore.setState({
    ...workbenchLayoutInitialState,
    terminalWorkspaceRatios: {},
  })
})

afterEach(() => {
  useContextMenuStore.setState({
    request: null,
    x: 0,
    y: 0,
    availabilityRevision: 0,
  })
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
      </div>,
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
      </div>,
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
      </div>,
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
    expect(useWorkbenchLayoutStore.getState().terminalGlobalRatio).toBeCloseTo(746 / 790 - 0.02)
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
    expect(useWorkbenchLayoutStore.getState().terminalWorkspaceRatios["/workspace"]).toBeCloseTo(
      0.5,
    )
    expect(useWorkbenchLayoutStore.getState().terminalGlobalRatio).toBe(0.4)

    fireEvent.pointerDown(handle, { button: 0, clientY: 505, pointerId: 2 })
    fireEvent.pointerMove(handle, { clientY: 426, pointerId: 2 })
    fireEvent.lostPointerCapture(handle, { pointerId: 2 })
    expect(useWorkbenchLayoutStore.getState().terminalWorkspaceRatios["/workspace"]).toBeCloseTo(
      0.6,
    )
    expect(screen.queryByText(/Main .* Terminal/)).not.toBeInTheDocument()
  })

  it("collapses to the 38px header and restores the effective ratio after expand and rail hide/show", () => {
    const containerRef = createRef<HTMLDivElement>()
    const view = render(
      <div ref={containerRef} data-testid="terminal-test-stack">
        <TerminalDrawer visible containerRef={containerRef} mainSurfaceMinHeight={44} />
      </div>,
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
        <TerminalDrawer visible={false} containerRef={containerRef} mainSurfaceMinHeight={44} />
      </div>,
    )
    expect(drawer.style.height).toBe("0px")
    view.rerender(
      <div ref={containerRef} data-testid="terminal-test-stack">
        <TerminalDrawer visible containerRef={containerRef} mainSurfaceMinHeight={44} />
      </div>,
    )
    expect(drawer.style.height).toBe("237px")
    expect(useWorkbenchLayoutStore.getState().terminalGlobalRatio).toBe(0.3)
  })
})

it("header 與 empty state 不會開啟 terminal entity menu", () => {
  renderDrawer()
  fireEvent.contextMenu(screen.getByTestId("terminal-header"))
  fireEvent.contextMenu(screen.getByText(i18n.t("noSessions", { ns: "terminal" })))
  expect(useContextMenuStore.getState().request).toBeNull()
})

describe("TerminalDrawer sessions", () => {
  it("keeps the workspace path out of the compact header while passing the raw path to the terminal", () => {
    const rawPath = "\\\\?\\C:\\Users\\Yuuzu\\專案 空間 #100%"
    useWorkspaceStore.setState({ workspacePath: rawPath })
    renderDrawer()

    expect(screen.queryByText("C:\\Users\\Yuuzu\\專案 空間 #100%")).toBeNull()
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
    expect(secondTerminal).toHaveAttribute("data-workspace", secondWorkspace)
    expect(secondTerminal).toBeVisible()
    expect(useTerminalStore.getState().sessionsForWorkspace(firstWorkspace)).toEqual([firstSession])

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

  it("suppresses the terminal viewport context menu without opening Yuzora actions", () => {
    useWorkspaceStore.setState({ workspacePath: "/workspace" })
    renderDrawer({ includeAppShellContextMenu: true })
    fireEvent.click(screen.getByTitle("New terminal"))

    const content = document.querySelector(".yzs.overflow-y-auto.font-mono") as HTMLElement
    const contentEvent = new MouseEvent("contextmenu", { bubbles: true, cancelable: true })
    expect(content.dispatchEvent(contentEvent)).toBe(false)
    expect(contentEvent.defaultPrevented).toBe(true)

    const terminal = screen.getByTestId(/terminal-session-/)
    let tuiSawDefaultPrevented: boolean | undefined
    terminal.addEventListener(
      "contextmenu",
      (event) => {
        tuiSawDefaultPrevented = event.defaultPrevented
      },
      { once: true },
    )
    const tuiEvent = new MouseEvent("contextmenu", { bubbles: true, cancelable: true })
    expect(terminal.dispatchEvent(tuiEvent)).toBe(false)
    expect(tuiSawDefaultPrevented).toBe(false)
    expect(tuiEvent.defaultPrevented).toBe(true)

    const pane = useTerminalStore.getState().layouts["/workspace"].panes[0]
    const paneEvent = new MouseEvent("contextmenu", { bubbles: true, cancelable: true })
    expect(screen.getByTestId(`terminal-pane-${pane.paneId}`).dispatchEvent(paneEvent)).toBe(false)
    expect(paneEvent.defaultPrevented).toBe(true)

    expect(useContextMenuStore.getState().request).toBeNull()
  })

  it("removes the fake Dock toolbar action", () => {
    renderDrawer()
    expect(screen.queryByTitle("Dock terminal into editor")).not.toBeInTheDocument()
  })

  it("passes the persisted shell override to new terminal sessions", () => {
    localStorage.setItem(
      "yuzora:terminal-settings",
      JSON.stringify({ shellPath: "/opt/homebrew/bin/fish", shellArgs: "" }),
    )
    useWorkspaceStore.setState({ workspacePath: "/workspace" })
    renderDrawer()

    fireEvent.click(screen.getByTitle("New terminal"))

    expect(screen.getByTestId(/terminal-session-/)).toHaveAttribute(
      "data-shell",
      "/opt/homebrew/bin/fish",
    )
    expect(useTerminalStore.getState().sessionsForWorkspace("/workspace")[0]?.shell).toBe(
      "/opt/homebrew/bin/fish",
    )
  })

  it("passes persisted shell args to new terminal sessions", () => {
    localStorage.setItem(
      "yuzora:terminal-settings",
      JSON.stringify({ shellPath: "/bin/sh", shellArgs: "-c echo-ok" }),
    )
    useWorkspaceStore.setState({ workspacePath: "/workspace" })
    renderDrawer()

    fireEvent.click(screen.getByTitle("New terminal"))

    expect(screen.getByTestId(/terminal-session-/)).toHaveAttribute("data-shell-args", "-c|echo-ok")
    expect(useTerminalStore.getState().sessionsForWorkspace("/workspace")[0]).toMatchObject({
      shellArgs: ["-c", "echo-ok"],
    })
  })

  it("opens a detected WSL distro profile with structured argv and snapshots TUI IME mode", async () => {
    localStorage.setItem(
      "yuzora:terminal-settings",
      JSON.stringify({
        defaultProfile: {
          id: "system",
          name: "System default",
          shell: "",
          args: [],
          kind: "system",
          cwdStrategy: "native",
        },
        customProfile: {
          id: "custom",
          name: "Custom",
          shell: "",
          args: [],
          kind: "custom",
          cwdStrategy: "native",
        },
        imeAnchorMode: "tui",
      }),
    )
    mockIPC((cmd) => {
      if (cmd === "pty_list_profiles") {
        return [
          {
            id: "wsl:Ubuntu",
            name: "WSL: Ubuntu",
            shell: "C:\\Windows\\System32\\wsl.exe",
            args: ["--distribution", "Ubuntu"],
            kind: "wsl",
            cwdStrategy: "wsl",
          },
        ]
      }
      return undefined
    })
    useWorkspaceStore.setState({ workspacePath: "/workspace" })
    renderDrawer()

    await act(async () => {
      fireEvent.pointerDown(screen.getByRole("button", { name: "New terminal with profile" }), {
        button: 0,
        ctrlKey: false,
      })
    })
    const menu = await screen.findByRole("menu")
    await act(async () => {
      fireEvent.click(within(menu).getByRole("menuitem", { name: "WSL: Ubuntu" }))
    })

    expect(screen.getByTestId(/terminal-session-/)).toHaveAttribute(
      "data-shell",
      "C:\\Windows\\System32\\wsl.exe",
    )
    expect(screen.getByTestId(/terminal-session-/)).toHaveAttribute(
      "data-shell-args",
      "--distribution|Ubuntu",
    )
    expect(screen.getByTestId(/terminal-session-/)).toHaveAttribute("data-ime-anchor", "tui")
  })

  it("closes the active PTY through the shared command and returns to the empty state", async () => {
    useWorkspaceStore.setState({ workspacePath: "/workspace" })
    renderDrawer()

    fireEvent.click(screen.getByTitle("New terminal"))
    fireEvent.click(screen.getByRole("button", { name: "Close Terminal 1" }))

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

  it("splits only to the right, caps visible panes at two, and keeps New unlimited", () => {
    useWorkspaceStore.setState({ workspacePath: "/workspace" })
    renderDrawer()

    fireEvent.click(screen.getByTitle("New terminal"))
    fireEvent.click(screen.getByTitle("Split right"))

    expect(screen.getAllByTestId(/terminal-session-/)).toHaveLength(2)
    expect(screen.getByTitle("A maximum of two terminals can be visible")).toBeDisabled()
    expect(screen.queryByTitle("Split down")).not.toBeInTheDocument()
    expect(screen.getByTitle("New terminal")).toBeEnabled()

    fireEvent.click(screen.getByTitle("New terminal"))
    expect(screen.getAllByRole("tab")).toHaveLength(3)
    expect(screen.getAllByTestId(/terminal-session-/)).toHaveLength(3)
    expect(useTerminalStore.getState().layouts["/workspace"].panes).toHaveLength(2)
    expect(
      screen
        .getAllByTestId(/terminal-session-/)
        .filter((node) => node.getAttribute("data-visible") === "true"),
    ).toHaveLength(2)
  })

  it("keeps the other split pane visible when New replaces only the focused pane", () => {
    useWorkspaceStore.setState({ workspacePath: "/workspace" })
    renderDrawer()

    fireEvent.click(screen.getByTitle("New terminal"))
    fireEvent.click(screen.getByTitle("Split right"))
    const [first, second] = useTerminalStore.getState().sessionsForWorkspace("/workspace")

    fireEvent.click(screen.getByTitle("New terminal"))
    const third = useTerminalStore.getState().sessionsForWorkspace("/workspace")[2]
    const layout = useTerminalStore.getState().layouts["/workspace"]

    expect(layout.panes.map((pane) => pane.sessionId)).toEqual([first.sessionId, third.sessionId])
    expect(screen.getByTestId(`terminal-session-${first.sessionId}`)).toHaveAttribute(
      "data-visible",
      "true",
    )
    expect(screen.getByTestId(`terminal-session-${second.sessionId}`)).toHaveAttribute(
      "data-visible",
      "false",
    )
    expect(screen.getByTestId(`terminal-session-${third.sessionId}`)).toHaveAttribute(
      "data-active",
      "true",
    )
  })

  it("selects hidden tabs into the focused pane and focuses already-visible tabs in place", () => {
    useWorkspaceStore.setState({ workspacePath: "/workspace" })
    renderDrawer()

    fireEvent.click(screen.getByTitle("New terminal"))
    fireEvent.click(screen.getByTitle("Split right"))
    fireEvent.click(screen.getByTitle("New terminal"))

    fireEvent.click(screen.getByRole("tab", { name: "Terminal 2" }))
    expect(
      useTerminalStore.getState().layouts["/workspace"].panes.map((pane) => pane.sessionId),
    ).toEqual(
      useTerminalStore
        .getState()
        .sessionsForWorkspace("/workspace")
        .slice(0, 2)
        .map((session) => session.sessionId),
    )
    expect(screen.getByRole("tab", { name: "Terminal 2" })).toHaveAttribute("aria-selected", "true")

    fireEvent.click(screen.getByRole("tab", { name: "Terminal 1" }))
    expect(screen.getByRole("tab", { name: "Terminal 1" })).toHaveAttribute("aria-selected", "true")
    expect(screen.getByRole("tab", { name: "Terminal 2" })).toHaveAttribute(
      "data-visible-pane",
      "right",
    )
  })

  it("lists every tab in All terminals with pane/focus markers and selects hidden entries", async () => {
    useWorkspaceStore.setState({ workspacePath: "/workspace" })
    renderDrawer()
    fireEvent.click(screen.getByTitle("New terminal"))
    fireEvent.click(screen.getByTitle("Split right"))
    fireEvent.click(screen.getByTitle("New terminal"))

    await act(async () => {
      fireEvent.pointerDown(screen.getByRole("button", { name: "All terminals" }), {
        button: 0,
        ctrlKey: false,
      })
    })
    const menu = await screen.findByRole("menu")
    expect(
      within(menu).getByRole("menuitem", { name: "Terminal 1 · Left" }),
    ).toBeInTheDocument()
    expect(within(menu).getByRole("menuitem", { name: "Terminal 2" })).toBeInTheDocument()
    expect(
      within(menu).getByRole("menuitem", { name: "Terminal 3 · Right · Focused" }),
    ).toBeInTheDocument()

    await act(async () => {
      fireEvent.click(within(menu).getByRole("menuitem", { name: "Terminal 2" }))
    })
    expect(screen.getByRole("tab", { name: "Terminal 2" })).toHaveAttribute(
      "aria-selected",
      "true",
    )
    expect(screen.getByRole("tab", { name: "Terminal 2" })).toHaveAttribute(
      "data-visible-pane",
      "right",
    )
  })

  it("keeps tabs mounted while collapsed and expands for tab, New, and Split actions", () => {
    useWorkspaceStore.setState({ workspacePath: "/workspace" })
    renderDrawer()

    fireEvent.click(screen.getByTitle("New terminal"))
    fireEvent.click(screen.getByRole("button", { name: "Collapse terminal" }))
    expect(screen.getByRole("tab", { name: "Terminal 1" })).toBeVisible()
    expect(screen.getByTestId(/terminal-session-/)).toHaveAttribute("data-visible", "false")

    fireEvent.click(screen.getByRole("tab", { name: "Terminal 1" }))
    expect(screen.getByRole("button", { name: "Collapse terminal" })).toBeInTheDocument()

    fireEvent.click(screen.getByRole("button", { name: "Collapse terminal" }))
    fireEvent.click(screen.getByTitle("New terminal"))
    expect(screen.getAllByRole("tab")).toHaveLength(2)
    expect(screen.getByRole("button", { name: "Collapse terminal" })).toBeInTheDocument()

    fireEvent.click(screen.getByRole("button", { name: "Collapse terminal" }))
    fireEvent.click(screen.getByTitle("Split right"))
    expect(screen.getAllByRole("tab")).toHaveLength(3)
    expect(useTerminalStore.getState().layouts["/workspace"].panes).toHaveLength(2)
    expect(screen.getByRole("button", { name: "Collapse terminal" })).toBeInTheDocument()
  })

  it("supports shell-driven titles and inline manual rename priority, clear, and cancel", () => {
    useWorkspaceStore.setState({ workspacePath: "/workspace" })
    renderDrawer()
    fireEvent.click(screen.getByTitle("New terminal"))

    const session = useTerminalStore.getState().sessionsForWorkspace("/workspace")[0]
    act(() => terminalSessionMocks.props.get(session.sessionId)?.onTitleChange?.("  vite dev  "))
    expect(screen.getByRole("tab", { name: "vite dev" })).toBeInTheDocument()

    fireEvent.doubleClick(screen.getByRole("tab", { name: "vite dev" }))
    const renameInput = screen.getByRole("textbox", {
      name: "Rename vite dev",
    })
    expect(renameInput).toHaveValue("vite dev")
    fireEvent.change(renameInput, { target: { value: "Frontend" } })
    fireEvent.keyDown(renameInput, { key: "Enter" })
    expect(screen.getByRole("tab", { name: "Frontend" })).toBeInTheDocument()

    act(() => terminalSessionMocks.props.get(session.sessionId)?.onTitleChange?.("server ready"))
    expect(screen.getByRole("tab", { name: "Frontend" })).toBeInTheDocument()

    fireEvent.doubleClick(screen.getByRole("tab", { name: "Frontend" }))
    const clearInput = screen.getByRole("textbox", { name: "Rename Frontend" })
    fireEvent.change(clearInput, { target: { value: "   " } })
    fireEvent.blur(clearInput)
    expect(screen.getByRole("tab", { name: "server ready" })).toBeInTheDocument()

    fireEvent.doubleClick(screen.getByRole("tab", { name: "server ready" }))
    const cancelInput = screen.getByRole("textbox", {
      name: "Rename server ready",
    })
    fireEvent.change(cancelInput, { target: { value: "Do not save" } })
    fireEvent.keyDown(cancelInput, { key: "Escape" })
    expect(screen.getByRole("tab", { name: "server ready" })).toBeInTheDocument()
  })

  it("opens a dedicated tab menu and reorders tabs without changing visible panes", () => {
    useWorkspaceStore.setState({ workspacePath: "/workspace" })
    renderDrawer()
    fireEvent.click(screen.getByTitle("New terminal"))
    fireEvent.click(screen.getByTitle("Split right"))
    fireEvent.click(screen.getByTitle("New terminal"))

    const firstTab = screen.getByRole("tab", { name: "Terminal 1" })
    const thirdTab = screen.getByRole("tab", { name: "Terminal 3" })
    fireEvent.contextMenu(firstTab)
    expect(useContextMenuStore.getState().request).toMatchObject({
      kind: "terminalTab",
      workspacePath: "/workspace",
      sessionId: firstTab.getAttribute("data-session-id"),
    })

    const panesBefore = useTerminalStore.getState().layouts["/workspace"].panes
    let draggedId = ""
    const dataTransfer = {
      effectAllowed: "none",
      setData: (_type: string, value: string) => {
        draggedId = value
      },
      getData: () => draggedId,
    }
    fireEvent.dragStart(firstTab, { dataTransfer })
    fireEvent.dragOver(thirdTab, { dataTransfer })
    fireEvent.drop(thirdTab, { dataTransfer })

    expect(screen.getAllByRole("tab").map((tab) => tab.getAttribute("aria-label"))).toEqual([
      "Terminal 2",
      "Terminal 3",
      "Terminal 1",
    ])
    expect(useTerminalStore.getState().layouts["/workspace"].panes).toEqual(panesBefore)
  })

  it("uses Arrow keys and Home/End only on the tab row", () => {
    useWorkspaceStore.setState({ workspacePath: "/workspace" })
    renderDrawer()
    fireEvent.click(screen.getByTitle("New terminal"))
    fireEvent.click(screen.getByTitle("New terminal"))
    fireEvent.click(screen.getByTitle("New terminal"))

    const tabList = screen.getByRole("tablist", { name: "Terminal tabs" })
    fireEvent.keyDown(tabList, { key: "ArrowLeft" })
    expect(screen.getByRole("tab", { name: "Terminal 2" })).toHaveAttribute("aria-selected", "true")
    fireEvent.keyDown(tabList, { key: "Home" })
    expect(screen.getByRole("tab", { name: "Terminal 1" })).toHaveAttribute("aria-selected", "true")
    fireEvent.keyDown(tabList, { key: "End" })
    expect(screen.getByRole("tab", { name: "Terminal 3" })).toHaveAttribute("aria-selected", "true")
  })

  it("closes hidden tabs without disturbing panes and unsplits when a visible tab closes", async () => {
    mockIPC((cmd) => (cmd === "pty_activity" ? "idle" : undefined))
    useWorkspaceStore.setState({ workspacePath: "/workspace" })
    renderDrawer()
    fireEvent.click(screen.getByTitle("New terminal"))
    fireEvent.click(screen.getByTitle("Split right"))
    fireEvent.click(screen.getByTitle("New terminal"))

    const sessions = useTerminalStore.getState().sessionsForWorkspace("/workspace")
    const panesBefore = useTerminalStore.getState().layouts["/workspace"].panes
    fireEvent.click(screen.getByRole("button", { name: "Close Terminal 2" }))
    await waitFor(() => expect(screen.queryByRole("tab", { name: "Terminal 2" })).toBeNull())
    expect(useTerminalStore.getState().layouts["/workspace"].panes).toEqual(panesBefore)

    fireEvent.click(screen.getByRole("button", { name: "Close Terminal 3" }))
    await waitFor(() => {
      expect(useTerminalStore.getState().layouts["/workspace"].panes).toHaveLength(1)
    })
    expect(useTerminalStore.getState().layouts["/workspace"].panes[0].sessionId).toBe(
      sessions[0].sessionId,
    )
  })

  it("resizes the fixed right split by keyboard and pointer and retains the workspace ratio", () => {
    useWorkspaceStore.setState({ workspacePath: "/workspace" })
    renderDrawer()
    fireEvent.click(screen.getByTitle("New terminal"))
    fireEvent.click(screen.getByTitle("Split right"))

    const divider = screen.getByRole("separator", {
      name: "Resize terminal panes",
    })
    fireEvent.keyDown(divider, { key: "ArrowRight" })
    expect(useTerminalStore.getState().layouts["/workspace"].splitRatio).toBeCloseTo(0.52)
    expect(divider).toHaveAttribute("aria-valuetext", "Left 52% · Right 48%")
    fireEvent.keyDown(divider, { key: "ArrowLeft", shiftKey: true })
    expect(useTerminalStore.getState().layouts["/workspace"].splitRatio).toBeCloseTo(0.42)

    const grid = screen.getByTestId("terminal-pane-grid")
    vi.spyOn(grid, "getBoundingClientRect").mockReturnValue({
      ...rect(193, 0),
      width: 1000,
      right: 1000,
    })
    fireEvent.pointerDown(divider, { button: 0, clientX: 420, pointerId: 7 })
    fireEvent.pointerMove(divider, { clientX: 750, pointerId: 7 })
    expect(divider).toHaveAttribute("aria-valuetext", "Left 75% · Right 25%")
    fireEvent.pointerUp(divider, { clientX: 750, pointerId: 7 })
    expect(useTerminalStore.getState().layouts["/workspace"].splitRatio).toBeCloseTo(0.75)
  })

  it("marks spawn failures, keeps the error tab selected, and closes it without activity probing", async () => {
    const commands: string[] = []
    mockIPC((cmd) => {
      commands.push(cmd)
      return undefined
    })
    useWorkspaceStore.setState({ workspacePath: "/workspace" })
    renderDrawer()
    fireEvent.click(screen.getByTitle("New terminal"))
    const session = useTerminalStore.getState().sessionsForWorkspace("/workspace")[0]

    act(() => terminalSessionMocks.props.get(session.sessionId)?.onOpenError?.("spawn failed"))
    expect(screen.getByLabelText("Terminal failed to start")).toBeInTheDocument()
    expect(screen.getByRole("tab", { name: "Terminal 1" })).toHaveAttribute("aria-selected", "true")

    fireEvent.click(screen.getByRole("button", { name: "Close Terminal 1" }))
    await waitFor(() => expect(screen.queryByRole("tab", { name: "Terminal 1" })).toBeNull())
    expect(commands).toContain("pty_close")
    expect(commands).not.toContain("pty_activity")
  })

  it("removes a naturally exited tab directly without a close prompt", () => {
    const commands: string[] = []
    mockIPC((cmd) => {
      commands.push(cmd)
      return undefined
    })
    useWorkspaceStore.setState({ workspacePath: "/workspace" })
    renderDrawer()
    fireEvent.click(screen.getByTitle("New terminal"))
    const session = useTerminalStore.getState().sessionsForWorkspace("/workspace")[0]

    act(() => terminalSessionMocks.props.get(session.sessionId)?.onExit?.(0))
    expect(screen.queryByRole("tab", { name: "Terminal 1" })).toBeNull()
    expect(commands).not.toContain("pty_activity")
    expect(commands).not.toContain("plugin:dialog|message")
  })
})

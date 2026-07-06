import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"
import { fireEvent, render, screen, waitFor, within } from "@testing-library/react"
import { clearMocks, mockIPC, mockWindows } from "@tauri-apps/api/mocks"

import { AppShell } from "@/app/AppShell"
import { useContextMenuStore } from "@/state/contextMenuStore"
import { uiInitialState, useUiStore } from "@/state/uiStore"
import { useWorkspaceStore } from "@/state/workspaceStore"

const windowMocks = vi.hoisted(() => ({
  closeHandlers: [] as Array<(event: { preventDefault: () => void }) => void | Promise<void>>,
  setTheme: vi.fn(() => Promise.resolve()),
  onCloseRequested: vi.fn(
    (handler: (event: { preventDefault: () => void }) => void | Promise<void>) => {
      windowMocks.closeHandlers.push(handler)
      return Promise.resolve(() => {})
    }
  ),
}))

vi.mock("@tauri-apps/api/window", () => ({
  getCurrentWindow: () => ({
    setTheme: windowMocks.setTheme,
    onCloseRequested: windowMocks.onCloseRequested,
  }),
}))

vi.mock("@/features/logs/userAction", () => ({
  logUserAction: vi.fn(async () => undefined),
}))

const MAC_UA =
  "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15 (KHTML, like Gecko)"

afterEach(() => {
  clearMocks()
  vi.clearAllMocks()
  windowMocks.closeHandlers.length = 0
  delete (globalThis as { isTauri?: boolean }).isTauri
  // 移除測試蓋上的 own property，讓 jsdom 原本的 prototype getter 復原
  delete (window.navigator as { userAgent?: string }).userAgent
  useContextMenuStore.setState({ kind: null, x: 0, y: 0, payload: {} })
})

describe("AppShell", () => {
  beforeEach(() => {
    useUiStore.setState(uiInitialState)
    useWorkspaceStore.setState({
      workspacePath: null,
      groups: [{ tabs: [], activePath: null }],
      activeGroupIndex: 0,
      pendingReveal: null,
    })
  })

  it("macOS 的 Tauri 內：渲染單一全寬頂部拖曳帶，內容列讓出頂部空間", () => {
    ;(globalThis as { isTauri?: boolean }).isTauri = true
    Object.defineProperty(window.navigator, "userAgent", { value: MAC_UA, configurable: true })
    mockWindows("main")
    mockIPC(() => {})

    const { container } = render(<AppShell />)

    const dragRegions = container.querySelectorAll("[data-tauri-drag-region]")
    expect(dragRegions).toHaveLength(1)
    expect(dragRegions[0]).toHaveClass("h-[20px]")
    expect(container.querySelector(".pt-\\[20px\\]")).toBeInTheDocument()
  })

  it("窄視窗自動收合 nav、放寬後自動展開，且手動操作優先", () => {
    ;(globalThis as { isTauri?: boolean }).isTauri = true
    mockWindows("main")
    mockIPC(() => {})

    const origW = window.innerWidth
    const navHidden = () =>
      (screen.getByLabelText("Project navigation").parentElement as HTMLElement).getAttribute(
        "aria-hidden"
      )

    try {
      window.innerWidth = 1200
      render(<AppShell />)
      expect(navHidden()).toBe("false") // 寬視窗：展開

      window.innerWidth = 700
      fireEvent(window, new Event("resize"))
      expect(navHidden()).toBe("true") // 越過門檻變窄：自動收合

      window.innerWidth = 1200
      fireEvent(window, new Event("resize"))
      expect(navHidden()).toBe("false") // 放寬：自動展開（僅還原 auto 收合）

      // 手動收合後，窄→寬循環不應自動展開（手動優先）
      fireEvent.click(screen.getByRole("button", { name: "Toggle sidebar" }))
      expect(navHidden()).toBe("true")
      window.innerWidth = 700
      fireEvent(window, new Event("resize"))
      window.innerWidth = 1200
      fireEvent(window, new Event("resize"))
      expect(navHidden()).toBe("true") // 仍維持手動收合
    } finally {
      window.innerWidth = origW
    }
  })

  it("Windows 的 Tauri 內：沒有拖曳帶也沒有頂部位移", () => {
    ;(globalThis as { isTauri?: boolean }).isTauri = true
    Object.defineProperty(window.navigator, "userAgent", {
      value: "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko)",
      configurable: true,
    })
    mockWindows("main")
    mockIPC(() => {})

    const { container } = render(<AppShell />)

    expect(container.querySelector("[data-tauri-drag-region]")).toBeNull()
    expect(container.querySelector(".pt-\\[20px\\]")).toBeNull()
  })

  it("在 Tauri 內將原生視窗 theme 同步為 app 主題（避免深色系統畫出黑邊框）", async () => {
    ;(globalThis as { isTauri?: boolean }).isTauri = true
    mockWindows("main")
    mockIPC(() => {})

    render(<AppShell />)

    await waitFor(() => expect(windowMocks.setTheme).toHaveBeenCalledWith("light"))
  })

  it("renders the rail, nav panel and status bar", () => {
    render(<AppShell />)

    expect(screen.getByLabelText("Workspace rail")).toBeInTheDocument()
    expect(screen.getByLabelText("Project navigation")).toBeInTheDocument()
    expect(screen.getByLabelText("Status bar")).toBeInTheDocument()
  })

  it("switches to Git mode and shows the selected state", () => {
    render(<AppShell />)

    const gitTab = screen.getByRole("tab", { name: "Git" })
    fireEvent.click(gitTab)

    expect(gitTab).toHaveAttribute("aria-selected", "true")
    expect(screen.getByTestId("nav-mode-content-git")).toBeInTheDocument()
  })

  it("collapses and restores the nav panel via the rail toggle", () => {
    render(<AppShell />)

    const toggle = screen.getByRole("button", { name: "Toggle sidebar" })
    // 收合是動畫（width→0 + opacity→0），面板保持 mounted，
    // 以 aria-hidden + inert 對輔助科技與互動隱藏。
    const nav = screen.getByLabelText("Project navigation")
    expect(nav.closest('[aria-hidden="true"]')).toBeNull()

    fireEvent.click(toggle)
    expect(nav.closest('[aria-hidden="true"]')).not.toBeNull()
    expect(nav.closest("[inert]")).not.toBeNull()

    fireEvent.click(toggle)
    expect(nav.closest('[aria-hidden="true"]')).toBeNull()
  })

  it("resizes the nav panel by dragging its workspace-area handle, without lag, clamped to min/max", () => {
    const { container } = render(<AppShell />)

    const nav = screen.getByLabelText("Project navigation").parentElement as HTMLElement
    // Disambiguate from TerminalDrawer's own (row-resize) drag handle, which
    // shares the same title but lives in the workspace column, not here.
    const handle = container.querySelector(
      '.cursor-col-resize[title="Drag to resize"]'
    ) as HTMLElement
    expect(nav).toHaveStyle({ width: "266px" })
    expect(nav.className).toMatch(/transition-\[width,opacity\]/)

    fireEvent.pointerDown(handle, { clientX: 0, pointerId: 1 })
    // Same lag fix as the terminal drag: the width transition is dropped
    // from the class list while actively resizing so it tracks 1:1.
    expect(nav.className).not.toMatch(/transition-\[width,opacity\]/)

    fireEvent.pointerMove(handle, { clientX: 80, pointerId: 1 })
    expect(nav).toHaveStyle({ width: "346px" })

    fireEvent.pointerMove(handle, { clientX: 300, pointerId: 1 })
    expect(nav).toHaveStyle({ width: "420px" }) // clamped to MAX_NAV_WIDTH

    fireEvent.pointerMove(handle, { clientX: -400, pointerId: 1 })
    expect(nav).toHaveStyle({ width: "220px" }) // clamped to MIN_NAV_WIDTH

    fireEvent.pointerUp(handle, { clientX: -400, pointerId: 1 })
    expect(nav.className).toMatch(/transition-\[width,opacity\]/)
  })

  it("opens the settings dialog from the rail avatar", async () => {
    render(<AppShell />)

    fireEvent.click(screen.getByRole("button", { name: "Settings" }))

    expect(await screen.findByRole("dialog")).toBeInTheDocument()
    expect(screen.getByRole("heading", { name: "Settings" })).toBeInTheDocument()
  })

  it("opens the command palette with Cmd+K and switches mode on selection", async () => {
    render(<AppShell />)

    fireEvent.keyDown(window, { key: "k", metaKey: true })

    const dialog = await screen.findByRole("dialog")
    fireEvent.click(within(dialog).getByText("Git"))

    expect(screen.queryByRole("dialog")).not.toBeInTheDocument()
    expect(screen.getByRole("tab", { name: "Git" })).toHaveAttribute("aria-selected", "true")
  })

  it("toggles the terminal drawer with Ctrl+`", () => {
    render(<AppShell />)

    const terminalDrawer = screen.getByText("Terminal").closest('[aria-hidden="true"]')
    expect(terminalDrawer).not.toBeNull()

    fireEvent.keyDown(window, { key: "`", ctrlKey: true })
    expect(screen.getByText("Terminal").closest('[aria-hidden="true"]')).toBeNull()

    fireEvent.keyDown(window, { key: "`", ctrlKey: true })
    expect(screen.getByText("Terminal").closest('[aria-hidden="true"]')).not.toBeNull()
  })

  it("best-effort closes pty and dev-server workspace on Tauri close request", async () => {
    ;(globalThis as { isTauri?: boolean }).isTauri = true
    mockWindows("main")
    const calls: string[] = []
    mockIPC((cmd) => {
      calls.push(cmd)
      if (cmd === "list_dir") return []
      return null
    })
    useWorkspaceStore.setState({ workspacePath: "/workspace" })

    render(<AppShell />)

    await waitFor(() => expect(windowMocks.onCloseRequested).toHaveBeenCalled())
    const preventDefault = vi.fn()
    await windowMocks.closeHandlers[0]({ preventDefault })

    expect(preventDefault).not.toHaveBeenCalled()
    await waitFor(() => {
      expect(calls).toContain("pty_close_workspace")
      expect(calls).toContain("dev_server_stop_workspace")
    })
  })

  it("keeps the EditorPanel container mounted (CSS-hidden) when switching away from Files mode and back", () => {
    render(<AppShell />)

    const editorEmptyState = () => screen.getByText("Open a project to start editing")
    expect(editorEmptyState().closest(".hidden")).toBeNull()

    fireEvent.click(screen.getByRole("tab", { name: "Git" }))
    expect(editorEmptyState()).toBeInTheDocument()
    expect(editorEmptyState().closest(".hidden")).not.toBeNull()

    fireEvent.click(screen.getByRole("tab", { name: "Files" }))
    expect(editorEmptyState().closest(".hidden")).toBeNull()
  })

  it("右鍵 root 開啟 general 選單並攔截原生選單，Escape 關閉", () => {
    mockWindows("main")
    mockIPC(() => {})
    const { container } = render(<AppShell />)

    const rootEl = container.firstElementChild as HTMLElement
    const nativeMenuShown = fireEvent.contextMenu(rootEl)

    // preventDefault 已被呼叫（fireEvent 回傳 false 代表 default 被擋掉）
    expect(nativeMenuShown).toBe(false)
    const menu = screen.getByTestId("context-menu")
    expect(menu.dataset.kind).toBe("general")

    fireEvent.keyDown(window, { key: "Escape" })
    expect(screen.queryByTestId("context-menu")).toBeNull()
  })
})

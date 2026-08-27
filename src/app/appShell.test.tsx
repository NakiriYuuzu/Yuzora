import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"
import { fireEvent, render, screen, waitFor, within } from "@testing-library/react"
import { clearMocks, mockIPC, mockWindows } from "@tauri-apps/api/mocks"

import { AppShell } from "@/app/AppShell"
import { APPEARANCE_SETTINGS_STORAGE_KEY } from "@/app/workbench/settingsStorage"
import type { SaveDirtyTabOutcome } from "@/editor/saveDocument"
import { useConfirmDialogStore } from "@/state/confirmDialogStore"
import { useContextMenuStore } from "@/state/contextMenuStore"
import {
  MOVE_OPENED_WORKSPACE_TO_TOP_STORAGE_KEY,
  useRecentWorkspacesStore,
} from "@/state/recentWorkspaces"
import { uiInitialState, useUiStore } from "@/state/uiStore"
import { useUpdateStore } from "@/state/updateStore"
import { useWorkspaceStore } from "@/state/workspaceStore"

const windowMocks = vi.hoisted(() => ({
  closeHandlers: [] as Array<(event: { preventDefault: () => void }) => void | Promise<void>>,
  setTheme: vi.fn(() => Promise.resolve()),
  show: vi.fn(() => Promise.resolve()),
  onCloseRequested: vi.fn(
    (handler: (event: { preventDefault: () => void }) => void | Promise<void>) => {
      windowMocks.closeHandlers.push(handler)
      return Promise.resolve(() => {})
    }
  ),
}))

const updaterMocks = vi.hoisted(() => ({
  check: vi.fn(async () => null as { version: string } | null),
}))

vi.mock("@tauri-apps/api/window", () => ({
  getCurrentWindow: () => ({
    setTheme: windowMocks.setTheme,
    show: windowMocks.show,
    onCloseRequested: windowMocks.onCloseRequested,
  }),
}))

vi.mock("@tauri-apps/plugin-updater", () => ({
  check: updaterMocks.check,
}))

vi.mock("@/features/logs/userAction", () => ({
  logUserAction: vi.fn(async () => undefined),
}))

const saveMocks = vi.hoisted(() => ({
  saveDirtyTab: vi.fn(async () => ({ kind: "saved" }) as SaveDirtyTabOutcome),
}))

vi.mock("@/editor/saveDocument", async (importOriginal) => ({
  ...(await importOriginal<typeof import("@/editor/saveDocument")>()),
  saveDirtyTab: saveMocks.saveDirtyTab,
}))

const MAC_UA =
  "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15 (KHTML, like Gecko)"

// The Bun-hosted test runtime injects an empty `localStorage` global with no
// Storage methods (see gitStore.test.ts). AppShell now persists the theme
// preference, so install a minimal in-memory Storage for real assertions.
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

afterEach(() => {
  clearMocks()
  vi.clearAllMocks()
  windowMocks.closeHandlers.length = 0
  delete (globalThis as { isTauri?: boolean }).isTauri
  // 移除測試蓋上的 own property，讓 jsdom 原本的 prototype getter 復原
  delete (window.navigator as { userAgent?: string }).userAgent
  useContextMenuStore.setState({ request: null, x: 0, y: 0, availabilityRevision: 0 })
})

describe("AppShell", () => {
  beforeEach(() => {
    // theme effect 會把偏好寫回 localStorage；測試間重裝＋清空避免跨測試殘留。
    installLocalStorage()
    localStorage.removeItem(APPEARANCE_SETTINGS_STORAGE_KEY)
    useRecentWorkspacesStore.setState({ moveOpenedWorkspaceToTop: true })
    useUiStore.setState(uiInitialState)
    useUpdateStore.getState().reset()
    updaterMocks.check.mockResolvedValue(null)
    saveMocks.saveDirtyTab.mockReset().mockResolvedValue({ kind: "saved" })
    useConfirmDialogStore.setState({ pending: null })
    useWorkspaceStore.setState({
      workspacePath: null,
      groups: [{ tabs: [], activePath: null }],
      activeGroupIndex: 0,
      pendingReveal: null,
    })
  })

  it("macOS 的 Tauri 內：渲染單一頂部拖曳帶但避開左上紅綠燈區，內容列讓出頂部空間", () => {
    ;(globalThis as { isTauri?: boolean }).isTauri = true
    Object.defineProperty(window.navigator, "userAgent", { value: MAC_UA, configurable: true })
    mockWindows("main")
    mockIPC(() => {})

    const { container } = render(<AppShell />)

    const dragRegions = container.querySelectorAll("[data-tauri-drag-region]")
    expect(dragRegions).toHaveLength(1)
    expect(dragRegions[0]).toHaveClass("h-[20px]")
    expect(dragRegions[0]).toHaveClass("left-20")
    expect(dragRegions[0]).not.toHaveClass("inset-x-0")
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

  it("在 Tauri 內將原生視窗 theme 同步為 app 主題：預設 auto → setTheme(null) 跟隨系統", async () => {
    ;(globalThis as { isTauri?: boolean }).isTauri = true
    mockWindows("main")
    mockIPC(() => {})

    render(<AppShell />)

    await waitFor(() => expect(windowMocks.setTheme).toHaveBeenCalledWith(null))
  })

  it("已持久化 light 偏好時重啟以 light 初始化並同步原生視窗", async () => {
    ;(globalThis as { isTauri?: boolean }).isTauri = true
    mockWindows("main")
    mockIPC(() => {})
    localStorage.setItem(APPEARANCE_SETTINGS_STORAGE_KEY, JSON.stringify({ theme: "light" }))

    render(<AppShell />)

    await waitFor(() => expect(windowMocks.setTheme).toHaveBeenCalledWith("light"))
    expect(document.documentElement.classList.contains("dark")).toBe(false)
  })

  it("隱藏啟動的視窗於首個主題化 frame 顯示（show 恰好一次，且等 setTheme 的 invoke resolve）", async () => {
    ;(globalThis as { isTauri?: boolean }).isTauri = true
    mockWindows("main")
    mockIPC(() => {})
    // setTheme 保持 pending：show 必須等原生端套用主題（await invoke），
    // 不能只靠 JS 呼叫順序——否則 race 時首個可見 frame 仍是 OS 主題。
    let resolveSetTheme!: () => void
    windowMocks.setTheme.mockImplementationOnce(
      () => new Promise<void>((resolve) => {
        resolveSetTheme = resolve
      })
    )

    render(<AppShell />)

    await waitFor(() => expect(windowMocks.setTheme).toHaveBeenCalled())
    await new Promise((resolve) => setTimeout(resolve, 0))
    expect(windowMocks.show).not.toHaveBeenCalled()

    resolveSetTheme()
    await waitFor(() => expect(windowMocks.show).toHaveBeenCalledTimes(1))

    // 之後的主題切換不再重複 show。
    fireEvent.click(screen.getByRole("button", { name: "Settings" }))
    const dialog = await screen.findByRole("dialog")
    fireEvent.click(within(dialog).getByRole("radio", { name: "Dark" }))
    await new Promise((resolve) => setTimeout(resolve, 0))
    expect(windowMocks.show).toHaveBeenCalledTimes(1)
  })

  it("setTheme 失敗仍 show（寧可主題錯也不留隱形視窗）", async () => {
    ;(globalThis as { isTauri?: boolean }).isTauri = true
    mockWindows("main")
    mockIPC(() => {})
    windowMocks.setTheme.mockImplementationOnce(() => Promise.reject(new Error("no permission")))

    render(<AppShell />)

    await waitFor(() => expect(windowMocks.show).toHaveBeenCalledTimes(1))
  })

  it("視窗可互動後只做一次背景更新檢查，更新狀態不在 Settings 外顯示", async () => {
    ;(globalThis as { isTauri?: boolean }).isTauri = true
    mockWindows("main")
    mockIPC(() => {})
    updaterMocks.check.mockResolvedValue({ version: "0.0.3" })

    let resolveShow!: () => void
    windowMocks.show.mockImplementationOnce(
      () =>
        new Promise<void>((resolve) => {
          resolveShow = resolve
        })
    )

    render(<AppShell />)

    await waitFor(() => expect(windowMocks.show).toHaveBeenCalledTimes(1))
    expect(updaterMocks.check).not.toHaveBeenCalled()

    resolveShow()
    await waitFor(() => expect(updaterMocks.check).toHaveBeenCalledTimes(1))
    expect(screen.queryByText("Yuzora v0.0.3 is available")).not.toBeInTheDocument()

    fireEvent.click(screen.getByRole("button", { name: "Settings" }))
    const dialog = await screen.findByRole("dialog")
    fireEvent.click(within(dialog).getByRole("radio", { name: "Dark" }))
    await new Promise((resolve) => setTimeout(resolve, 0))
    expect(updaterMocks.check).toHaveBeenCalledTimes(1)
  })

  it("背景檢查失敗時全域靜默，只記錄去敏感診斷事件", async () => {
    ;(globalThis as { isTauri?: boolean }).isTauri = true
    mockWindows("main")
    mockIPC(() => {})
    updaterMocks.check.mockRejectedValue(
      new Error("https://updates.invalid/?token=never-log-this")
    )
    const warn = vi.spyOn(console, "warn").mockImplementation(() => {})

    try {
      render(<AppShell />)

      await waitFor(() => expect(updaterMocks.check).toHaveBeenCalledTimes(1))
      await waitFor(() =>
        expect(warn).toHaveBeenCalledWith("Update check failed", {
          event: "update_check_failed",
        })
      )
      expect(screen.queryByText("Couldn't check for updates")).not.toBeInTheDocument()
      expect(JSON.stringify(warn.mock.calls)).not.toContain("never-log-this")
    } finally {
      warn.mockRestore()
    }
  })

  it("預設 auto 且系統為深色時套用 dark class，並把偏好寫回 localStorage", () => {
    const matchMediaSpy = vi.spyOn(window, "matchMedia").mockImplementation(
      (query: string) =>
        ({
          matches: true,
          media: query,
          onchange: null,
          addListener: () => {},
          removeListener: () => {},
          addEventListener: () => {},
          removeEventListener: () => {},
          dispatchEvent: () => false,
        }) as unknown as MediaQueryList
    )
    try {
      render(<AppShell />)

      expect(document.documentElement.classList.contains("dark")).toBe(true)
    expect(localStorage.getItem(APPEARANCE_SETTINGS_STORAGE_KEY)).toBe(
      JSON.stringify({ theme: "auto", accent: "lime" })
      )
    } finally {
      matchMediaSpy.mockRestore()
    }
  })

  it("在 Settings 切換主題後把新偏好寫回 localStorage", async () => {
    render(<AppShell />)

    fireEvent.click(screen.getByRole("button", { name: "Settings" }))
    const dialog = await screen.findByRole("dialog")
    fireEvent.click(within(dialog).getByRole("radio", { name: "Dark" }))

    expect(document.documentElement.classList.contains("dark")).toBe(true)
    expect(localStorage.getItem(APPEARANCE_SETTINGS_STORAGE_KEY)).toBe(
      JSON.stringify({ theme: "dark", accent: "lime" })
    )
  })

  it("切換 accent 後立即套用完整 token 並持久化", async () => {
    render(<AppShell />)

    fireEvent.click(screen.getByRole("button", { name: "Settings" }))
    const dialog = await screen.findByRole("dialog")
    fireEvent.click(within(dialog).getByRole("radio", { name: "blue" }))

    expect(document.documentElement.style.getPropertyValue("--yz-accent")).toBe("#2f6bff")
    expect(document.documentElement.style.getPropertyValue("--yz-accent-rgb")).toBe("47, 107, 255")
    expect(document.documentElement.style.getPropertyValue("--yz-accent-ink")).toBe("#2456cc")
    expect(localStorage.getItem(APPEARANCE_SETTINGS_STORAGE_KEY)).toBe(
      JSON.stringify({ theme: "auto", accent: "blue" })
    )
  })

  it("重啟時從持久化 appearance 還原 accent", () => {
    localStorage.setItem(
      APPEARANCE_SETTINGS_STORAGE_KEY,
      JSON.stringify({ theme: "light", accent: "violet" })
    )

    render(<AppShell />)

    expect(document.documentElement.style.getPropertyValue("--yz-accent")).toBe("#7b5bff")
    expect(document.documentElement.style.getPropertyValue("--yz-accent-rgb")).toBe("123, 91, 255")
    expect(document.documentElement.style.getPropertyValue("--yz-accent-ink")).toBe("#5d3fd3")
  })

  it("可在 Settings 關閉切換專案時移至最上方並立即持久化", async () => {
    render(<AppShell />)

    fireEvent.click(screen.getByRole("button", { name: "Settings" }))
    const dialog = await screen.findByRole("dialog")
    const toggle = within(dialog).getByRole("switch", { name: "Move opened workspace to top" })

    expect(toggle).toBeChecked()
    fireEvent.click(toggle)

    expect(toggle).not.toBeChecked()
    expect(useRecentWorkspacesStore.getState().moveOpenedWorkspaceToTop).toBe(false)
    expect(localStorage.getItem(MOVE_OPENED_WORKSPACE_TO_TOP_STORAGE_KEY)).toBe("false")
  })

  it("renders the rail, nav panel and status bar", () => {
    render(<AppShell />)

    expect(screen.getByLabelText("Workspace rail")).toBeInTheDocument()
    expect(screen.getByLabelText("Project navigation")).toBeInTheDocument()
    expect(screen.getByLabelText("Status bar")).toBeInTheDocument()
  })

  it("keeps the shared 44px main-surface floor in ADE and Files modes", () => {
    render(<AppShell />)
    const mainSurface = screen.getByTestId("main-surface")

    // ADE is the default mode and shares the editor surface floor with Files.
    expect(screen.getByRole("tab", { name: "ADE" })).toHaveAttribute("aria-selected", "true")
    expect(mainSurface).toHaveStyle({ minHeight: "44px" })
    fireEvent.click(screen.getByRole("tab", { name: "Files" }))
    expect(mainSurface).toHaveStyle({ minHeight: "44px" })
    fireEvent.click(screen.getByRole("tab", { name: "ADE" }))
    expect(mainSurface).toHaveStyle({ minHeight: "44px" })
    expect(screen.getByTestId("nav-mode-content-ade")).toBeInTheDocument()
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

    const terminalDrawer = screen.getByTestId("terminal-drawer")
    expect(terminalDrawer).toHaveAttribute("aria-hidden", "true")

    fireEvent.keyDown(window, { key: "`", ctrlKey: true })
    expect(terminalDrawer).toHaveAttribute("aria-hidden", "false")

    fireEvent.keyDown(window, { key: "`", ctrlKey: true })
    expect(terminalDrawer).toHaveAttribute("aria-hidden", "true")
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

  // issue #21：Alt+F4 / 標題列關閉鈕走 WINDOW_CLOSE_REQUESTED，handler 會被
  // @tauri-apps/api await，preventDefault() 會取消 destroy。Yuzora 不保留草稿，
  // 因此這是未儲存內容的最後一道關卡。
  describe("關閉視窗時的未儲存攔截", () => {
    const openCloseRequest = () => {
      ;(globalThis as { isTauri?: boolean }).isTauri = true
      mockWindows("main")
      const calls: Array<{ cmd: string; payload: unknown }> = []
      mockIPC((cmd, payload) => {
        calls.push({ cmd, payload })
        if (cmd === "list_dir") return []
        // The dirty tab below makes EditorPane actually load the document.
        if (cmd === "open_file") return { kind: "full", content: "", size: 0, lineEnding: "lf" }
        return null
      })
      useWorkspaceStore.setState({
        workspacePath: "/workspace",
        activeGroupIndex: 0,
        groups: [
          {
            activePath: "/workspace/a.ts",
            tabs: [
              { path: "/workspace/a.ts", name: "a.ts", dirty: true, externallyModified: false },
            ],
          },
        ],
        pendingReveal: null,
      })
      return { calls, names: () => calls.map((c) => c.cmd) }
    }

    const fireCloseRequest = async () => {
      await waitFor(() => expect(windowMocks.onCloseRequested).toHaveBeenCalled())
      const preventDefault = vi.fn()
      const closing = windowMocks.closeHandlers[0]({ preventDefault })
      await waitFor(() => expect(useConfirmDialogStore.getState().pending).not.toBeNull())
      return { preventDefault, closing }
    }

    it("選取消：preventDefault 阻止關閉，dirty 分頁與子行程都維持原狀", async () => {
      const { names } = openCloseRequest()
      render(<AppShell />)

      const { preventDefault, closing } = await fireCloseRequest()
      useConfirmDialogStore.getState().respond("cancel")
      await closing

      expect(preventDefault).toHaveBeenCalled()
      expect(names()).not.toContain("pty_close_workspace")
      expect(names()).not.toContain("dev_server_stop_workspace")
      expect(saveMocks.saveDirtyTab).not.toHaveBeenCalled()
      expect(useWorkspaceStore.getState().groups[0].tabs[0].dirty).toBe(true)
    })

    it("選不儲存：放行關閉並清理 PTY 與 dev server", async () => {
      const { names } = openCloseRequest()
      render(<AppShell />)

      const { preventDefault, closing } = await fireCloseRequest()
      useConfirmDialogStore.getState().respond("discard")
      await closing

      expect(preventDefault).not.toHaveBeenCalled()
      expect(saveMocks.saveDirtyTab).not.toHaveBeenCalled()
      expect(names()).toContain("pty_close_workspace")
      expect(names()).toContain("dev_server_stop_workspace")
    })

    it("選儲存且成功：存完才放行關閉", async () => {
      const { names } = openCloseRequest()
      render(<AppShell />)

      const { preventDefault, closing } = await fireCloseRequest()
      useConfirmDialogStore.getState().respond("save")
      await closing

      expect(preventDefault).not.toHaveBeenCalled()
      expect(saveMocks.saveDirtyTab).toHaveBeenCalledWith("/workspace/a.ts")
      expect(names()).toContain("pty_close_workspace")
    })

    // issue #21 的原始情境：mixed-EOL 檔案被儲存檢查擋下。存檔失敗卻照樣關閉
    // 等於永久丟掉使用者內容。
    it("選儲存但存檔失敗：preventDefault，app 保持開啟", async () => {
      const { names } = openCloseRequest()
      saveMocks.saveDirtyTab.mockResolvedValue({ kind: "blocked", reason: "mixed" })
      render(<AppShell />)

      const { preventDefault, closing } = await fireCloseRequest()
      useConfirmDialogStore.getState().respond("save")
      await closing

      expect(preventDefault).toHaveBeenCalled()
      expect(names()).not.toContain("pty_close_workspace")
      expect(useWorkspaceStore.getState().groups[0].tabs[0].dirty).toBe(true)
    })

    // guard 一旦 reject 而沒被接住，wrapper 的 await 會炸掉：既不 preventDefault
    // 也不 destroy，視窗會變成按了沒反應、沒對話框也沒錯誤。fail-safe 是「當成
    // 取消」——寧可關不掉也不能靜默丟內容。
    it("dirty gate 自己爆炸時 fail-safe 成取消，不會靜默關窗", async () => {
      const { names } = openCloseRequest()
      saveMocks.saveDirtyTab.mockRejectedValue(new Error("boom"))
      const warn = vi.spyOn(console, "warn").mockImplementation(() => {})

      try {
        render(<AppShell />)

        const { preventDefault, closing } = await fireCloseRequest()
        useConfirmDialogStore.getState().respond("save")
        await closing

        expect(preventDefault).toHaveBeenCalled()
        expect(names()).not.toContain("pty_close_workspace")
        expect(warn).toHaveBeenCalledWith("unsaved-changes guard failed:", expect.any(Error))
      } finally {
        warn.mockRestore()
      }
    })

    it("尚未開啟工作區時仍註冊關閉攔截", async () => {
      ;(globalThis as { isTauri?: boolean }).isTauri = true
      mockWindows("main")
      mockIPC(() => {})

      render(<AppShell />)

      await waitFor(() => expect(windowMocks.onCloseRequested).toHaveBeenCalled())
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

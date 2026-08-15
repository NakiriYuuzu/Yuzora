import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"
import { act, cleanup, render, waitFor } from "@testing-library/react"
import indexHtml from "../../index.html?raw"

import { getDocument } from "@/editor/documentRegistry"
import { allowWorkspaceAssetScope } from "@/lib/ipc"
import { openWorkspaceAtPath } from "@/lib/workspaceActions"
import { SessionRestoreBridge } from "@/workbench/SessionRestoreBridge"
import { markdownPreviewPath } from "@/lib/markdownPreviewTab"
import {
  WORKSPACE_SESSION_STORAGE_KEY,
  loadWorkspaceSession,
  loadWorkspaceSessionEntry,
  saveWorkspaceSession,
  type WorkspaceSession
} from "@/state/workspaceSession"
import {
  PREVIEW_TAB_PATH,
  useWorkspaceStore
} from "@/state/workspaceStore"

vi.mock("@/lib/workspaceActions", () => ({
  openWorkspaceAtPath: vi.fn(),
  pickWorkspace: vi.fn()
}))

vi.mock("@/editor/documentRegistry", () => ({
  getDocument: vi.fn()
}))

// The bridge awaits the asset-scope grant before opening restored image tabs
// (NB-1). Only allowWorkspaceAssetScope is consumed from the ipc surface here.
vi.mock("@/lib/ipc", () => ({
  allowWorkspaceAssetScope: vi.fn()
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
    }
  }
  Object.defineProperty(globalThis, "localStorage", {
    value: mock,
    configurable: true,
    writable: true
  })
}

const tabPaths = () => useWorkspaceStore.getState().groups[0].tabs.map((t) => t.path)
const activePath = () => useWorkspaceStore.getState().groups[0].activePath

// Default openWorkspaceAtPath mock: mirror the real side effect (setWorkspace,
// which resets the store to an empty group) so the ref-gate race is exercised.
function mockOpenResolves() {
  vi.mocked(openWorkspaceAtPath).mockImplementation(async (path: string) => {
    useWorkspaceStore.getState().setWorkspace(path)
    return true
  })
}

beforeEach(() => {
  installLocalStorage()
  localStorage.clear()
  useWorkspaceStore.setState({
    workspacePath: null,
    groups: [{ tabs: [], activePath: null }],
    activeGroupIndex: 0,
    pendingReveal: null
  })
  vi.mocked(openWorkspaceAtPath).mockReset()
  vi.mocked(getDocument).mockReset().mockResolvedValue({} as never)
  vi.mocked(allowWorkspaceAssetScope).mockReset().mockResolvedValue(undefined)
})

afterEach(() => {
  cleanup()
  vi.useRealTimers()
})

const SESSION: WorkspaceSession = {
  workspacePath: "/ws",
  tabs: ["/ws/a.ts", "/ws/b.ts"],
  activePath: "/ws/a.ts"
}

// Startup splash contract: the bridge dismisses the index.html splash exactly
// when the restore attempt settles (or immediately when there is nothing to
// restore). `yz-splash-leave` marks dismissal without waiting the fade timer.
function insertSplash(): HTMLElement {
  const el = document.createElement("div")
  el.id = "yz-splash"
  document.body.appendChild(el)
  return el
}

const splashDismissed = (el: HTMLElement) =>
  !document.getElementById("yz-splash") || el.classList.contains("yz-splash-leave")

function runInlineBootScript(): void {
  const script = indexHtml.match(/<script>\s*([\s\S]*?)<\/script>/)?.[1]
  if (!script) throw new Error("index.html inline boot script not found")
  Function(script)()
}

describe("SessionRestoreBridge splash 退場", () => {
  afterEach(() => {
    document.getElementById("yz-splash")?.remove()
  })

  it("無可還原 session 時 mount 即退場 splash", async () => {
    const el = insertSplash()

    render(<SessionRestoreBridge />)

    await waitFor(() => expect(splashDismissed(el)).toBe(true))
    expect(openWorkspaceAtPath).not.toHaveBeenCalled()
  })

  it("有 session 時等還原 settle 才退場 splash", async () => {
    saveWorkspaceSession(SESSION)
    let resolveOpen!: () => void
    vi.mocked(openWorkspaceAtPath).mockImplementation(
      (path: string) =>
        new Promise<boolean>((resolve) => {
          resolveOpen = () => {
            useWorkspaceStore.getState().setWorkspace(path)
            resolve(true)
          }
        })
    )
    const el = insertSplash()

    render(<SessionRestoreBridge />)

    // openWorkspaceAtPath 進行中：splash 必須還在。
    await waitFor(() => expect(openWorkspaceAtPath).toHaveBeenCalled())
    expect(splashDismissed(el)).toBe(false)

    resolveOpen()
    await waitFor(() => expect(splashDismissed(el)).toBe(true))
  })

  it("慢速文件還原超過 4 秒時仍保持 splash，直到所有文件 ready", async () => {
    vi.useFakeTimers()
    saveWorkspaceSession({
      workspacePath: "/ws",
      tabs: ["/ws/a.ts"],
      activePath: null
    })
    mockOpenResolves()
    let resolveFirstDocument!: () => void
    vi.mocked(getDocument)
      .mockImplementationOnce(
        () =>
          new Promise((resolve) => {
            resolveFirstDocument = () => resolve({} as never)
          })
      )
      .mockResolvedValue({} as never)
    const el = insertSplash()
    runInlineBootScript()

    render(<SessionRestoreBridge />)
    await act(async () => {
      await Promise.resolve()
    })
    expect(getDocument).toHaveBeenCalledWith("/ws/a.ts")

    act(() => vi.advanceTimersByTime(4_001))

    expect(splashDismissed(el)).toBe(false)

    await act(async () => {
      resolveFirstDocument()
      await Promise.resolve()
    })
    expect(splashDismissed(el)).toBe(true)
  })

  it("workspace 消失（還原拋錯）時仍退場，不留永久遮罩", async () => {
    saveWorkspaceSession(SESSION)
    vi.mocked(openWorkspaceAtPath).mockRejectedValue(new Error("gone"))
    const el = insertSplash()

    render(<SessionRestoreBridge />)

    await waitFor(() => expect(splashDismissed(el)).toBe(true))
    expect(loadWorkspaceSession()).toBeNull()
  })
})

describe("SessionRestoreBridge 還原", () => {
  it("有 session 時開啟 workspace、還原分頁與 active", async () => {
    saveWorkspaceSession(SESSION)
    mockOpenResolves()

    render(<SessionRestoreBridge />)

    await waitFor(() => expect(tabPaths()).toEqual(["/ws/a.ts", "/ws/b.ts"]))
    expect(activePath()).toBe("/ws/a.ts")
    // 冷啟路徑自帶逐檔驗證還原，須關掉 workspaceActions 的 map 還原以免
    // 失效檔案的分頁被搶先開出來（#60 T4c）。
    expect(openWorkspaceAtPath).toHaveBeenCalledWith("/ws", { restoreSessionTabs: false })
  })

  it("沒有 session 時完全不動（不呼叫 openWorkspaceAtPath）", async () => {
    render(<SessionRestoreBridge />)
    // Let any pending microtasks flush.
    await Promise.resolve()
    expect(openWorkspaceAtPath).not.toHaveBeenCalled()
    expect(useWorkspaceStore.getState().workspacePath).toBeNull()
  })

  it("已開啟 workspace 時略過還原", async () => {
    saveWorkspaceSession(SESSION)
    mockOpenResolves()
    useWorkspaceStore.setState({ workspacePath: "/already" })

    render(<SessionRestoreBridge />)
    await Promise.resolve()
    expect(openWorkspaceAtPath).not.toHaveBeenCalled()
  })

  it("資料夾消失時清掉 stale session 並優雅退回", async () => {
    saveWorkspaceSession(SESSION)
    vi.mocked(openWorkspaceAtPath).mockRejectedValue(new Error("folder gone"))

    render(<SessionRestoreBridge />)

    await waitFor(() => expect(loadWorkspaceSession()).toBeNull())
    expect(tabPaths()).toEqual([])
  })

  // T4 覆核修正（NB-1 回歸）：圖片分頁走 asset protocol，grant 未落地前開出
  // ImageView 會 403 進永久 loadError。冷啟還原含圖片分頁時必須先等 grant。
  it("冷啟還原含圖片分頁時，先等 asset scope grant 落地才開分頁", async () => {
    saveWorkspaceSession({
      workspacePath: "/ws",
      tabs: ["/ws/pic.png", "/ws/a.ts"],
      activePath: "/ws/pic.png"
    })
    mockOpenResolves()
    let releaseGrant!: () => void
    vi.mocked(allowWorkspaceAssetScope).mockImplementation(
      () =>
        new Promise<void>((resolve) => {
          releaseGrant = () => resolve()
        })
    )

    render(<SessionRestoreBridge />)

    await waitFor(() => expect(allowWorkspaceAssetScope).toHaveBeenCalledWith("/ws"))
    // grant 未落地：不得先開出任何分頁（ImageView 一 mount 就發 <img> 請求）。
    expect(tabPaths()).toEqual([])

    releaseGrant()
    await waitFor(() => expect(tabPaths()).toEqual(["/ws/pic.png", "/ws/a.ts"]))
    expect(activePath()).toBe("/ws/pic.png")
  })

  it("冷啟還原純文字分頁時不等 grant（不引入多餘序列化）", async () => {
    saveWorkspaceSession(SESSION)
    mockOpenResolves()
    vi.mocked(allowWorkspaceAssetScope).mockImplementation(() => new Promise<void>(() => {}))

    render(<SessionRestoreBridge />)

    await waitFor(() => expect(tabPaths()).toEqual(["/ws/a.ts", "/ws/b.ts"]))
    expect(allowWorkspaceAssetScope).not.toHaveBeenCalled()
  })

  it("還原時單一失效檔案靜默略過", async () => {
    saveWorkspaceSession({
      workspacePath: "/ws",
      tabs: ["/ws/a.ts", "/ws/missing.ts", "/ws/b.ts"],
      activePath: "/ws/a.ts"
    })
    mockOpenResolves()
    vi.mocked(getDocument).mockImplementation(async (path: string) => {
      if (path === "/ws/missing.ts") throw new Error("no such file")
      return {} as never
    })

    render(<SessionRestoreBridge />)

    await waitFor(() => expect(tabPaths()).toEqual(["/ws/a.ts", "/ws/b.ts"]))
  })
})

describe("SessionRestoreBridge ref gate", () => {
  it("還原的非同步 openWorkspaceAtPath resolve 前，存檔 effect 不得用空狀態覆寫 session", async () => {
    saveWorkspaceSession(SESSION)

    let resolveOpen: (() => void) | undefined
    vi.mocked(openWorkspaceAtPath).mockImplementation(
      (path: string) =>
        new Promise<boolean>((res) => {
          // setWorkspace resets groups to empty and fires the save subscription
          // while the restore is still mid-flight (gate must be closed).
          useWorkspaceStore.getState().setWorkspace(path)
          resolveOpen = () => res(true)
        })
    )

    render(<SessionRestoreBridge />)

    // openWorkspaceAtPath is pending; the store has already transitioned to the
    // empty workspace. The persisted session must still carry the tabs.
    await waitFor(() => expect(useWorkspaceStore.getState().workspacePath).toBe("/ws"))
    expect(loadWorkspaceSession()?.tabs).toEqual(["/ws/a.ts", "/ws/b.ts"])

    // Let the restore finish; tabs come back.
    resolveOpen?.()
    await waitFor(() => expect(tabPaths()).toEqual(["/ws/a.ts", "/ws/b.ts"]))
    expect(loadWorkspaceSession()?.tabs).toEqual(["/ws/a.ts", "/ws/b.ts"])
  })

  it("還原完成後，分頁變動會寫回 session（gate 已開）", async () => {
    saveWorkspaceSession(SESSION)
    mockOpenResolves()

    render(<SessionRestoreBridge />)
    await waitFor(() => expect(tabPaths()).toEqual(["/ws/a.ts", "/ws/b.ts"]))

    useWorkspaceStore.getState().openTab("/ws/c.ts")
    await waitFor(() =>
      expect(loadWorkspaceSession()?.tabs).toEqual(["/ws/a.ts", "/ws/b.ts", "/ws/c.ts"])
    )
  })

  it("workspace 切換過場（groups 清空）不得覆寫目標 workspace 的 entry，只推進 last 指標", async () => {
    saveWorkspaceSession({
      workspacePath: "/other",
      tabs: ["/other/x.ts"],
      activePath: "/other/x.ts"
    })
    saveWorkspaceSession(SESSION) // last = /ws（冷啟還原目標）
    mockOpenResolves()

    render(<SessionRestoreBridge />)
    await waitFor(() => expect(tabPaths()).toEqual(["/ws/a.ts", "/ws/b.ts"]))

    // 模擬 workspaceActions 切換到 /other：setWorkspace 讓 store 過場為空 groups。
    act(() => useWorkspaceStore.getState().setWorkspace("/other"))

    // /other 的 entry 必須毫髮無傷（workspaceActions 隨後靠它還原 tabs）。
    expect(loadWorkspaceSessionEntry("/other")).toEqual({
      tabs: ["/other/x.ts"],
      activePath: "/other/x.ts"
    })
    // last 指標推進：下次冷啟還原 /other。
    expect(loadWorkspaceSession()?.workspacePath).toBe("/other")
  })

  it("切換後的分頁變動寫入新 workspace 的 entry，舊 workspace entry 不動", async () => {
    saveWorkspaceSession(SESSION)
    mockOpenResolves()

    render(<SessionRestoreBridge />)
    await waitFor(() => expect(tabPaths()).toEqual(["/ws/a.ts", "/ws/b.ts"]))

    act(() => {
      useWorkspaceStore.getState().setWorkspace("/other")
      useWorkspaceStore.getState().openTab("/other/y.ts")
    })

    await waitFor(() =>
      expect(loadWorkspaceSessionEntry("/other")).toEqual({
        tabs: ["/other/y.ts"],
        activePath: "/other/y.ts"
      })
    )
    expect(loadWorkspaceSessionEntry("/ws")).toEqual({
      tabs: ["/ws/a.ts", "/ws/b.ts"],
      activePath: "/ws/a.ts"
    })
  })

  it("存檔時過濾 pseudo preview 分頁", async () => {
    saveWorkspaceSession(SESSION)
    mockOpenResolves()

    render(<SessionRestoreBridge />)
    await waitFor(() => expect(tabPaths()).toEqual(["/ws/a.ts", "/ws/b.ts"]))

    useWorkspaceStore.getState().openPreviewTab()
    await waitFor(() => expect(tabPaths()).toContain(PREVIEW_TAB_PATH))
    // The preview pseudo-tab becomes active, but neither the tab nor an active
    // pseudo-path may be persisted.
    expect(loadWorkspaceSession()?.tabs).toEqual(["/ws/a.ts", "/ws/b.ts"])
    expect(loadWorkspaceSession()?.activePath).toBeNull()
  })

  it("persists reordered group-0 file tabs and still excludes Herdr pages", async () => {
    saveWorkspaceSession(SESSION)
    mockOpenResolves()

    render(<SessionRestoreBridge />)
    await waitFor(() => expect(tabPaths()).toEqual(["/ws/a.ts", "/ws/b.ts"]))

    useWorkspaceStore.getState().openHerdrTerminalPage({
      herdrSessionId: "default",
      terminalId: "term-1",
      title: "Herdr",
      herdrTabId: "tab-1",
      herdrWorkspaceId: "ws-1"
    })
    useWorkspaceStore.getState().reorderTab(0, "/ws/a.ts", 1)

    await waitFor(() => expect(loadWorkspaceSession()?.tabs).toEqual(["/ws/b.ts", "/ws/a.ts"]))
    expect(loadWorkspaceSession()?.tabs.some((path) => path.startsWith("yuzora://"))).toBe(false)
  })

  it("cold restore never opens a markdown preview synthetic tab", async () => {
    const previewPath = markdownPreviewPath("/ws/notes.md")
    localStorage.setItem(
      WORKSPACE_SESSION_STORAGE_KEY,
      JSON.stringify({
        version: 2,
        lastWorkspacePath: "/ws",
        workspaces: {
          "/ws": {
            tabs: ["/ws/a.ts", previewPath, "/ws/notes.md"],
            activePath: previewPath
          }
        }
      })
    )
    mockOpenResolves()

    render(<SessionRestoreBridge />)

    await waitFor(() => expect(tabPaths()).toEqual(["/ws/a.ts", "/ws/notes.md"]))
    // Saved active was synthetic, so restore does not keep that path. The last
    // real file opened during restore becomes active instead.
    expect(activePath()).toBe("/ws/notes.md")
    expect(activePath()).not.toBe(previewPath)
    expect(getDocument).not.toHaveBeenCalledWith(previewPath)
    expect(
      useWorkspaceStore.getState().groups.some((group) =>
        group.tabs.some((tab) => tab.kind === "markdown-preview" || tab.path === previewPath)
      )
    ).toBe(false)
  })

  it("save filters markdown preview tabs and a synthetic active path", async () => {
    saveWorkspaceSession(SESSION)
    mockOpenResolves()

    render(<SessionRestoreBridge />)
    await waitFor(() => expect(tabPaths()).toEqual(["/ws/a.ts", "/ws/b.ts"]))

    const previewPath = markdownPreviewPath("/ws/notes.md")
    useWorkspaceStore.setState({
      workspacePath: "/ws",
      activeGroupIndex: 0,
      groups: [{
        activePath: previewPath,
        tabs: [
          { path: "/ws/a.ts", name: "a.ts", dirty: false, externallyModified: false },
          { path: "/ws/notes.md", name: "notes.md", dirty: false, externallyModified: false },
          {
            path: previewPath,
            name: "Preview",
            dirty: false,
            externallyModified: false,
            kind: "markdown-preview",
            sourcePath: "/ws/notes.md"
          }
        ]
      }]
    })

    await waitFor(() => {
      expect(loadWorkspaceSession()?.tabs).toEqual(["/ws/a.ts", "/ws/notes.md"])
    })
    expect(loadWorkspaceSession()?.activePath).toBeNull()
    expect(loadWorkspaceSession()?.tabs.some((path) => path.startsWith("yuzora://"))).toBe(false)
  })
})

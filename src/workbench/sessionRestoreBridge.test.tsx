import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"
import { cleanup, render, waitFor } from "@testing-library/react"

import { getDocument } from "@/editor/documentRegistry"
import { openWorkspaceAtPath } from "@/lib/workspaceActions"
import { SessionRestoreBridge } from "@/workbench/SessionRestoreBridge"
import {
  loadWorkspaceSession,
  saveWorkspaceSession,
  type WorkspaceSession
} from "@/state/workspaceSession"
import { PREVIEW_TAB_PATH, useWorkspaceStore } from "@/state/workspaceStore"

vi.mock("@/lib/workspaceActions", () => ({
  openWorkspaceAtPath: vi.fn(),
  pickWorkspace: vi.fn()
}))

vi.mock("@/editor/documentRegistry", () => ({
  getDocument: vi.fn()
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
})

afterEach(() => {
  cleanup()
})

const SESSION: WorkspaceSession = {
  workspacePath: "/ws",
  tabs: ["/ws/a.ts", "/ws/b.ts"],
  activePath: "/ws/a.ts"
}

describe("SessionRestoreBridge 還原", () => {
  it("有 session 時開啟 workspace、還原分頁與 active", async () => {
    saveWorkspaceSession(SESSION)
    mockOpenResolves()

    render(<SessionRestoreBridge />)

    await waitFor(() => expect(tabPaths()).toEqual(["/ws/a.ts", "/ws/b.ts"]))
    expect(activePath()).toBe("/ws/a.ts")
    expect(openWorkspaceAtPath).toHaveBeenCalledWith("/ws")
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
        new Promise<void>((res) => {
          // setWorkspace resets groups to empty and fires the save subscription
          // while the restore is still mid-flight (gate must be closed).
          useWorkspaceStore.getState().setWorkspace(path)
          resolveOpen = () => res()
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
})

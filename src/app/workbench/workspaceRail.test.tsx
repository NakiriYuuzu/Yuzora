import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"
import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react"

import { WorkspaceRail } from "@/app/workbench/WorkspaceRail"
import { openWorkspaceAtPath, pickWorkspace } from "@/lib/workspaceActions"
import type { SessionState } from "@/state/agentStore"
import { useAgentStore } from "@/state/agentStore"
import { useContextMenuStore } from "@/state/contextMenuStore"
import { useRecentWorkspacesStore } from "@/state/recentWorkspaces"
import { useWorkspaceStore } from "@/state/workspaceStore"

function session(overrides: Partial<SessionState> = {}): SessionState {
  return {
    title: "New session",
    agentId: "codex",
    agentLabel: "Codex",
    model: null,
    tone: "idle",
    transcript: [],
    availableCommands: [],
    stopReason: null,
    stopBadge: null,
    error: null,
    queueDepth: null,
    running: null,
    pendingTurn: false,
    metadataTitle: false,
    cwd: null,
    ...overrides
  }
}

vi.mock("@/lib/workspaceActions", () => ({
  openWorkspaceAtPath: vi.fn(),
  pickWorkspace: vi.fn()
}))

const MAC_UA =
  "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15 (KHTML, like Gecko)"

const renderRail = () =>
  render(
    <WorkspaceRail
      navCollapsed={false}
      onToggleNav={() => {}}
      onOpenSettings={() => {}}
      previewOpen={false}
      onTogglePreview={() => {}}
      terminalOpen={false}
      onToggleTerminalDrawer={() => {}}
    />
  )

// The Bun-hosted test runtime injects an empty `localStorage` global with no
// Storage methods (see gitStore.test.ts); install a minimal in-memory Storage
// so the recent-workspaces store can be exercised for real.
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

beforeEach(() => {
  installLocalStorage()
  localStorage.clear()
  useRecentWorkspacesStore.setState({ list: [] })
  useWorkspaceStore.setState({ workspacePath: null })
  vi.mocked(openWorkspaceAtPath).mockReset()
  vi.mocked(pickWorkspace).mockReset().mockResolvedValue(true)
})

afterEach(() => {
  cleanup()
  delete (globalThis as { isTauri?: boolean }).isTauri
  // 移除測試蓋上的 own property，讓 jsdom 原本的 prototype getter 復原
  delete (window.navigator as { userAgent?: string }).userAgent
  useContextMenuStore.setState({ request: null, x: 0, y: 0, availabilityRevision: 0 })
  useAgentStore.setState({ sessions: new Map() })
})

describe("WorkspaceRail 紅綠燈區塊", () => {
  it("不再渲染裝飾圓點或 drag region — 頂部空間由 AppShell 的標題帶統一讓出", () => {
    ;(globalThis as { isTauri?: boolean }).isTauri = true
    Object.defineProperty(window.navigator, "userAgent", { value: MAC_UA, configurable: true })

    const { container } = renderRail()

    expect(container.querySelector("[data-tauri-drag-region]")).toBeNull()
    expect(container.querySelector('[class*="ff5f57"]')).toBeNull()
  })
})

it("右鍵 rail 開啟 rail 選單", () => {
  const { container } = renderRail()
  fireEvent.contextMenu(container.querySelector("nav") as HTMLElement)
  expect(useContextMenuStore.getState().request?.kind).toBe("rail")
})

describe("WorkspaceRail 開啟 workspace 按鈕", () => {
  it("渲染開啟 workspace 的 + 按鈕", () => {
    renderRail()
    expect(screen.getByRole("button", { name: "Open workspace" })).toBeInTheDocument()
  })

  it("點擊 + 按鈕呼叫 pickWorkspace", () => {
    renderRail()
    fireEvent.click(screen.getByRole("button", { name: "Open workspace" }))
    expect(pickWorkspace).toHaveBeenCalledTimes(1)
  })
})

describe("WorkspaceRail RECENT tiles", () => {
  it("為每個最近 workspace 渲染 tile，glyph 為資料夾名首字", () => {
    useRecentWorkspacesStore.setState({ list: ["/Users/tester/projects/yuzora"] })
    renderRail()
    const tile = screen.getByRole("button", { name: "Open yuzora" })
    expect(tile).toBeInTheDocument()
    expect(tile).toHaveTextContent("Y")
    expect(tile).toHaveAttribute("title", "/Users/tester/projects/yuzora")
  })

  it("highlight 目前開啟的 workspace tile（trailing slash 也對齊）", () => {
    useRecentWorkspacesStore.setState({ list: ["/Users/tester/projects/yuzora"] })
    useWorkspaceStore.setState({ workspacePath: "/Users/tester/projects/yuzora/" })
    renderRail()
    expect(screen.getByRole("button", { name: "Open yuzora" })).toHaveAttribute(
      "aria-pressed",
      "true"
    )
  })

  it("點擊 tile 以該路徑呼叫 openWorkspaceAtPath", () => {
    vi.mocked(openWorkspaceAtPath).mockResolvedValue(undefined)
    useRecentWorkspacesStore.setState({ list: ["/Users/tester/projects/yuzora"] })
    renderRail()
    fireEvent.click(screen.getByRole("button", { name: "Open yuzora" }))
    expect(openWorkspaceAtPath).toHaveBeenCalledWith("/Users/tester/projects/yuzora")
  })

  it("以 safe name/title 顯示 extended UNC，alias active 仍對齊且 open target 保留 raw path", () => {
    const rawPath = "\\\\?\\UNC\\Server\\Share\\專案 空間"
    vi.mocked(openWorkspaceAtPath).mockResolvedValue(undefined)
    useRecentWorkspacesStore.setState({ list: [rawPath] })
    useWorkspaceStore.setState({ workspacePath: "\\\\server\\share\\專案 空間\\" })
    renderRail()

    const tile = screen.getByRole("button", { name: "Open 專案 空間" })
    expect(tile).toHaveAttribute("title", "\\\\Server\\Share\\專案 空間")
    expect(tile).toHaveAttribute("aria-pressed", "true")

    fireEvent.click(tile)
    expect(openWorkspaceAtPath).toHaveBeenCalledWith(rawPath)
  })

  it("開啟失敗時把 tile 從最近清單移除並顯示提示", async () => {
    vi.mocked(openWorkspaceAtPath).mockRejectedValue(new Error("gone"))
    useRecentWorkspacesStore.setState({ list: ["/Users/tester/projects/yuzora"] })
    renderRail()
    fireEvent.click(screen.getByRole("button", { name: "Open yuzora" }))
    await waitFor(() => {
      expect(useRecentWorkspacesStore.getState().list).toEqual([])
    })
    expect(screen.getByRole("status")).toBeInTheDocument()
  })

  it("沒有最近 workspace 時不渲染 RECENT 區塊", () => {
    renderRail()
    expect(screen.queryByText("Recent")).toBeNull()
  })
})

describe("WorkspaceRail agent 徽章", () => {
  it("有 agent session 的 tile 顯示總數徽章，且 running>0 時套用 accent 色", () => {
    useRecentWorkspacesStore.setState({ list: ["/Users/tester/projects/yuzora"] })
    useAgentStore.setState({
      sessions: new Map([
        ["a1", session({ cwd: "/Users/tester/projects/yuzora", running: true })],
        ["a2", session({ cwd: "/Users/tester/projects/yuzora", tone: "run" })]
      ])
    })
    renderRail()
    const tile = screen.getByRole("button", { name: "Open yuzora" })
    const badge = tile.parentElement?.querySelector("span")
    expect(badge).not.toBeNull()
    expect(badge).toHaveTextContent("2/2")
    expect(badge?.className).toContain("yz-accent")
  })

  it("total>0 但 running=0 時徽章套用灰色而非 accent 色", () => {
    useRecentWorkspacesStore.setState({ list: ["/Users/tester/projects/yuzora"] })
    useAgentStore.setState({
      sessions: new Map([["a1", session({ cwd: "/Users/tester/projects/yuzora" })]])
    })
    renderRail()
    const tile = screen.getByRole("button", { name: "Open yuzora" })
    const badge = tile.parentElement?.querySelector("span")
    expect(badge).toHaveTextContent("1")
    expect(badge?.className).toContain("ink-4")
    expect(badge?.className).not.toContain("yz-accent")
  })

  it("零 agent 的 tile 不渲染徽章", () => {
    useRecentWorkspacesStore.setState({ list: ["/Users/tester/projects/yuzora"] })
    renderRail()
    const tile = screen.getByRole("button", { name: "Open yuzora" })
    expect(tile.parentElement?.querySelector("span")).toBeNull()
  })

  it("依 normalizeWorkspacePath 對齊 — trailing slash 的 session cwd 仍計入同一 tile", () => {
    useRecentWorkspacesStore.setState({ list: ["/Users/tester/projects/yuzora"] })
    useAgentStore.setState({
      sessions: new Map([["a1", session({ cwd: "/Users/tester/projects/yuzora/" })]])
    })
    renderRail()
    const tile = screen.getByRole("button", { name: "Open yuzora" })
    expect(tile.parentElement?.querySelector("span")).toHaveTextContent("1")
  })

  it("F2: rail badge shows total and running with an accessible label", () => {
    useRecentWorkspacesStore.setState({ list: ["/Users/tester/projects/yuzora"] })
    useAgentStore.setState({
      sessions: new Map([
        ["a1", session({ cwd: "/Users/tester/projects/yuzora", running: true })],
        ["a2", session({ cwd: "/Users/tester/projects/yuzora" })]
      ])
    })
    renderRail()
    expect(screen.getByText("1/2")).toBeInTheDocument()
    expect(screen.getByLabelText("2 agents, 1 running")).toBeInTheDocument()
  })
})

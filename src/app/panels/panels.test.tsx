import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"
import { fireEvent, render, screen, within } from "@testing-library/react"

import { AppShell } from "@/app/AppShell"
import { GitPanel } from "@/app/panels/GitPanel"
import { PreviewPanel } from "@/app/panels/PreviewPanel"
import { GitNavContent } from "@/app/workbench/GitNavContent"
import { useContextMenuStore } from "@/state/contextMenuStore"
import { initialGitState, useGitStore } from "@/state/gitStore"
import { usePreviewStore } from "@/state/previewStore"
import { uiInitialState, useUiStore } from "@/state/uiStore"
import { useWorkspaceStore } from "@/state/workspaceStore"
import { useSftpStore } from "@/state/sftpStore"

const logMocks = vi.hoisted(() => ({
  logQueryCalls: [] as Array<Record<string, unknown>>,
  queryResult: [] as unknown[],
  sourcesResult: [] as string[],
}))

vi.mock("@/features/logs/logQuery", () => ({
  logSources: async () => logMocks.sourcesResult,
  logQuery: async (filters: unknown) => {
    logMocks.logQueryCalls.push({ filters })
    return logMocks.queryResult
  },
  logExport: vi.fn(async () => ({ path: "/tmp/yuzora-logs.zip", summary: null })),
  logSanitizeLines: vi.fn(async (lines: string[]) => lines),
  getLogLevel: async () => "info",
  setLogLevel: vi.fn(async () => undefined),
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
  logMocks.logQueryCalls = []
  logMocks.queryResult = []
  logMocks.sourcesResult = ["ui", "agent"]
})

afterEach(() => {
  useContextMenuStore.setState({ request: null, x: 0, y: 0, availabilityRevision: 0 })
  // gitStore persists across the module graph; environment set in one test
  // leaks into the next (e.g. the "No repository status" nav assertion relies
  // on a null environment). Reset to the initial snapshot after each test.
  useGitStore.setState(initialGitState)
  usePreviewStore.getState().reset()
  useUiStore.setState(uiInitialState)
  useWorkspaceStore.setState({ workspacePath: null })
  vi.clearAllMocks()
})

// Covers the Git/Database/SSH/Agent mode entry states (Task E2) plus the
// Settings dialog content. The mode switcher tablist is named
// "Workbench mode" (ProjectNavPanel) so it can be scoped precisely — some
// mode panels have their own internal tabs (e.g. SSH's "SSH" segment vs.
// the rail's "SSH" mode tab) that would otherwise collide on an unscoped
// getByRole("tab", { name: ... }) query.


describe("Git/Database/SSH/Agent mode entry states", () => {
  it("shows the git nav and enables all three git view tabs", () => {
    useGitStore.setState({
      environment: { status: "ready", root: "/w", version: "2.50.1" },
      status: {
        branch: "main", headOid: "0".repeat(40), detached: false, upstream: null,
        ahead: 0, behind: 0, staged: [], unstaged: [], untracked: [], conflicted: [],
        inProgress: null
      }
    })
    render(<AppShell />)
    fireEvent.click(screen.getByRole("button", {name:"Expand right workspace tools"}))
    fireEvent.mouseDown(screen.getByRole("tab", {name:"Git"}), {button:0,ctrlKey:false})
    fireEvent.click(screen.getByRole("button", {name:"History and branch graph"}))

    const nav = document.getElementById("workbench-tools")!
    expect(within(nav).getByText("Working tree clean")).toBeInTheDocument()

    // Log (default), Local changes and Console are all live now.
    const gitViews = screen.getByRole("tablist", { name: "Git views" })
    expect(within(gitViews).getByRole("tab", { name: /Log/ })).not.toBeDisabled()
    expect(within(gitViews).getByRole("tab", { name: "Local changes" })).not.toBeDisabled()
    expect(within(gitViews).getByRole("tab", { name: "Console" })).not.toBeDisabled()
  })

  it("shows the database nav and main entry states", () => {
    render(<AppShell />)
    fireEvent.click(screen.getByRole("button", {name:"Database"}))

    const nav = screen.getByLabelText("Database connections")
    expect(within(nav).getByText("No database connections")).toBeInTheDocument()
    expect(within(nav).getByRole("button", { name: "New connection…" })).toBeInTheDocument()
    expect(screen.getByText("Database connections are not configured")).toBeInTheDocument()
  })

  it("opens SFTP transfers from Files without a separate SSH mode", () => {
    useSftpStore.getState().setPanelOpen(false)
    render(<AppShell />)

    expect(screen.queryByRole("tab", { name: "SSH" })).not.toBeInTheDocument()
    fireEvent.click(screen.getByRole("button", {name:"SSH / SFTP"}))
    expect(useSftpStore.getState().panelOpen).toBe(true)
    expect(screen.getByTestId("main-surface")).toBeInTheDocument()
    useSftpStore.getState().setPanelOpen(false)
  })

  it("shows the ADE nav entry state (Herdr Spaces/Agents)", () => {
    render(<AppShell />)


    const nav = screen.getByRole("complementary", { name: "Sidebar navigation" })
    // Default herdrStore is idle/connecting until HerdrBridge bootstraps.
    expect(
      within(nav).getByText(/All shows loaded runtime namespaces only/)
    ).toBeInTheDocument()
  })
})

describe("Settings dialog content", () => {
  it("has the design nav sections and editor settings", async () => {
    render(<AppShell />)
    fireEvent.click(screen.getByRole("button", { name: "Settings" }))
    const dialog = await screen.findByRole("dialog")

    expect(within(dialog).getByRole("tab", { name: "Appearance" })).toBeInTheDocument()
    expect(within(dialog).getByRole("tab", { name: "Editor" })).toBeInTheDocument()
    expect(within(dialog).getByRole("tab", { name: "Safety" })).toBeInTheDocument()

    fireEvent.mouseDown(within(dialog).getByRole("tab", { name: "Editor" }))

    expect(within(dialog).getByRole("switch", { name: "Show minimap" })).toBeInTheDocument()
  })

  it("switches the document theme from the Appearance tab", async () => {
    render(<AppShell />)
    fireEvent.click(screen.getByRole("button", { name: "Settings" }))
    const dialog = await screen.findByRole("dialog")

    expect(document.documentElement).not.toHaveClass("dark")

    fireEvent.click(within(dialog).getByRole("radio", { name: "Dark" }))
    expect(document.documentElement).toHaveClass("dark")

    fireEvent.click(within(dialog).getByRole("radio", { name: "Light" }))
    expect(document.documentElement).not.toHaveClass("dark")
  })

  it("git section shows detection state and remote-check controls", async () => {
    useGitStore.setState({
      environment: { status: "ready", root: "/w", version: "2.50.1" },
      remoteCheck: { mode: "probe", intervalSec: 180 },
    })
    render(<AppShell />)
    fireEvent.click(screen.getByRole("button", { name: "Settings" }))
    const dialog = await screen.findByRole("dialog")

    fireEvent.mouseDown(within(dialog).getByRole("tab", { name: /^Git/ }))
    expect(within(dialog).getByText(/2\.50\.1/)).toBeInTheDocument()
    expect(within(dialog).getByRole("radio", { name: "Read-only check" })).toHaveAttribute(
      "aria-checked",
      "true"
    )

    fireEvent.click(within(dialog).getByRole("radio", { name: "Auto fetch" }))
    expect(useGitStore.getState().remoteCheck.mode).toBe("autofetch")
  })

  it("remote-check control exposes a single-choice radio group", async () => {
    useGitStore.setState({ environment: { status: "ready", root: "/w", version: "2.50.1" } })
    render(<AppShell />)
    fireEvent.click(screen.getByRole("button", { name: "Settings" }))
    const dialog = await screen.findByRole("dialog")
    fireEvent.mouseDown(within(dialog).getByRole("tab", { name: /^Git/ }))

    expect(within(dialog).getByRole("radiogroup", { name: "Remote checks" })).toBeInTheDocument()
    expect(within(dialog).getByRole("radio", { name: "Read-only check" })).toHaveAttribute("aria-checked", "true")
  })

  it("clamps the remote-check interval on blur and allows intermediate typing (T19)", async () => {
    useGitStore.setState({
      environment: { status: "ready", root: "/w", version: "2.50.1" },
      remoteCheck: { mode: "probe", intervalSec: 180 },
    })
    render(<AppShell />)
    fireEvent.click(screen.getByRole("button", { name: "Settings" }))
    const dialog = await screen.findByRole("dialog")
    fireEvent.mouseDown(within(dialog).getByRole("tab", { name: /^Git/ }))

    const input = within(dialog).getByRole("spinbutton") as HTMLInputElement
    // A sub-minimum keystroke is accepted while typing (no immediate rejection)…
    fireEvent.change(input, { target: { value: "4" } })
    expect(input.value).toBe("4")
    expect(useGitStore.getState().remoteCheck.intervalSec).toBe(180)
    // …and only clamps + commits on blur.
    fireEvent.blur(input)
    expect(useGitStore.getState().remoteCheck.intervalSec).toBe(30)
  })
})

describe("Git guided setup", () => {
  it("git panel shows guided setup when git missing", () => {
    useGitStore.setState({ environment: { status: "missing", reason: "git not found" } })
    render(<GitPanel />)
    expect(screen.getByText("Git not detected")).toBeInTheDocument()
    expect(screen.getByRole("button", { name: "Re-detect" })).toBeInTheDocument()
  })

  it("git nav shows guided setup when git missing", () => {
    useGitStore.setState({ environment: { status: "missing", reason: "git not found" } })
    render(<GitNavContent />)
    expect(screen.getByText("Git not detected")).toBeInTheDocument()
    expect(screen.getByRole("button", { name: "Re-detect" })).toBeInTheDocument()
  })
})

it("Preview root 保留原生右鍵；只有 Preview Chrome 開 Yuzora menu", () => {
  useWorkspaceStore.setState({ workspacePath: "/workspace" })
  render(<PreviewPanel />)

  expect(fireEvent.contextMenu(screen.getByTestId("preview-panel"))).toBe(true)
  expect(useContextMenuStore.getState().request).toBeNull()

  expect(fireEvent.contextMenu(screen.getByTestId("preview-toolbar"))).toBe(false)
  expect(useContextMenuStore.getState().request).toEqual({
    kind: "preview",
    workspacePath: "/workspace",
    url: null,
  })
})

it("右鍵 Git 面板開啟 git 選單", () => {
  useGitStore.setState({
    environment: { status: "ready", root: "/w", version: "2.50.1" },
    status: {
      branch: "main", headOid: "0".repeat(40), detached: false, upstream: null,
      ahead: 0, behind: 0, staged: [], unstaged: [], untracked: [], conflicted: [],
      inProgress: null
    }
  })
  render(<GitPanel />)
  // Default tab is Log; its details panel always shows this prompt.
  fireEvent.contextMenu(screen.getByText("Select a commit to view details"))
  expect(useContextMenuStore.getState().request?.kind).toBe("git")
})

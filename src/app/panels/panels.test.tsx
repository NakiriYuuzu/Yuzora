import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"
import { act, fireEvent, render, screen, waitFor, within } from "@testing-library/react"

import { AppShell } from "@/app/AppShell"
import { AgentZonePanel } from "@/app/panels/AgentZonePanel"
import { GitPanel } from "@/app/panels/GitPanel"
import { PreviewPanel } from "@/app/panels/PreviewPanel"
import { GitNavContent } from "@/app/workbench/GitNavContent"
import { SettingsDialog } from "@/app/workbench/SettingsDialog"
import { useContextMenuStore } from "@/state/contextMenuStore"
import { initialGitState, useGitStore } from "@/state/gitStore"
import { usePreviewStore } from "@/state/previewStore"
import { uiInitialState, useUiStore } from "@/state/uiStore"
import { useWorkspaceStore } from "@/state/workspaceStore"

const ipcMocks = vi.hoisted(() => ({
  devServerDetect: vi.fn(),
  devServerStart: vi.fn(),
  devServerStop: vi.fn(),
}))

const logMocks = vi.hoisted(() => ({
  logQueryCalls: [] as Array<Record<string, unknown>>,
  queryResult: [] as unknown[],
  sourcesResult: [] as string[],
}))

vi.mock("@/lib/ipc", async (importOriginal) => ({
  ...(await importOriginal<typeof import("@/lib/ipc")>()),
  devServerDetect: (...args: unknown[]) => ipcMocks.devServerDetect(...args),
  devServerStart: (...args: unknown[]) => ipcMocks.devServerStart(...args),
  devServerStop: (...args: unknown[]) => ipcMocks.devServerStop(...args),
}))

vi.mock("@/features/logs/logQuery", () => ({
  logSources: async () => logMocks.sourcesResult,
  logQuery: async (filters: unknown) => {
    logMocks.logQueryCalls.push({ filters })
    return logMocks.queryResult
  },
  logExport: vi.fn(async () => "/tmp/yuzora-logs.zip"),
}))

vi.mock("@/features/logs/userAction", () => ({
  logUserAction: vi.fn(async () => undefined),
}))

const previewLogRows = [
  {
    timestamp: "2026-01-02T05:06:07+08:00",
    level: "error",
    kind: "debug",
    source: "dev_server",
    workspace_path: "/workspace",
    event: "dev_server_start_failed",
    message: "command not found",
    metadata: { command: "bun run dev" },
  },
]

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
  logMocks.sourcesResult = ["dev_server", "lsp"]
})

afterEach(() => {
  useContextMenuStore.setState({ kind: null, x: 0, y: 0, payload: {} })
  // gitStore persists across the module graph; environment set in one test
  // leaks into the next (e.g. the "No repository status" nav assertion relies
  // on a null environment). Reset to the initial snapshot after each test.
  useGitStore.setState(initialGitState)
  usePreviewStore.getState().reset()
  useUiStore.setState(uiInitialState)
  useWorkspaceStore.setState({ workspacePath: null })
  vi.clearAllMocks()
})

function deferred<T>() {
  let resolve!: (value: T) => void
  let reject!: (error: unknown) => void
  const promise = new Promise<T>((res, rej) => {
    resolve = res
    reject = rej
  })
  return { promise, resolve, reject }
}

// Covers the Git/Database/SSH/Agent mode entry states (Task E2) plus the
// Settings dialog content. The mode switcher tablist is named
// "Workbench mode" (ProjectNavPanel) so it can be scoped precisely — some
// mode panels have their own internal tabs (e.g. SSH's "SSH" segment vs.
// the rail's "SSH" mode tab) that would otherwise collide on an unscoped
// getByRole("tab", { name: ... }) query.
function switchMode(name: string) {
  const modeSwitcher = screen.getByRole("tablist", { name: "Workbench mode" })
  fireEvent.click(within(modeSwitcher).getByRole("tab", { name }))
}

function PreviewWithSettings() {
  const open = useUiStore((s) => s.settingsOpen)
  const setOpen = useUiStore((s) => s.setSettingsOpen)
  const section = useUiStore((s) => s.settingsSection)
  const language = useUiStore((s) => s.settingsLanguage)
  const nonce = useUiStore((s) => s.settingsNonce)

  return (
    <>
      <PreviewPanel />
      <SettingsDialog
        open={open}
        onOpenChange={setOpen}
        theme="light"
        onThemeChange={() => {}}
        initialSection={section ?? undefined}
        initialLanguage={language ?? undefined}
        openNonce={nonce}
      />
    </>
  )
}

describe("Git/Database/SSH/Agent mode entry states", () => {
  it("shows the git nav and enables all three git view tabs", () => {
    render(<AppShell />)
    switchMode("Git")

    const nav = screen.getByTestId("nav-mode-content-git")
    expect(within(nav).getByText("No repository status")).toBeInTheDocument()

    // Log (default), Local changes and Console are all live now.
    const gitViews = screen.getByRole("tablist", { name: "Git views" })
    expect(within(gitViews).getByRole("tab", { name: /Log/ })).not.toBeDisabled()
    expect(within(gitViews).getByRole("tab", { name: "Local changes" })).not.toBeDisabled()
    expect(within(gitViews).getByRole("tab", { name: "Console" })).not.toBeDisabled()
  })

  it("shows the database nav and main entry states", () => {
    render(<AppShell />)
    switchMode("Database")

    const nav = screen.getByTestId("nav-mode-content-database")
    expect(within(nav).getByText("No database connections")).toBeInTheDocument()
    expect(within(nav).getByRole("button", { name: "New connection" })).toBeInTheDocument()
    expect(screen.getByText("Database connections are not configured")).toBeInTheDocument()
  })

  it("shows the ssh nav and main entry states, and switches the SFTP/SSH tabs", () => {
    render(<AppShell />)
    switchMode("SSH")

    const nav = screen.getByTestId("nav-mode-content-ssh")
    expect(within(nav).getByText("No hosts yet")).toBeInTheDocument()
    expect(within(nav).getByRole("button", { name: "New host" })).toBeInTheDocument()

    expect(screen.getAllByText("Remote sessions are not configured").length).toBeGreaterThan(0)
    expect(screen.getByText("Connect a host to transfer files here.")).toBeInTheDocument()

    // Radix's Tabs.Trigger switches on mousedown (not click) — see
    // @radix-ui/react-tabs's Trigger, which wires activation to onMouseDown
    // (plus onKeyDown/onFocus). fireEvent.click alone never fires that
    // handler, so tab-switching assertions use mouseDown here and below.
    const viewSwitcher = screen.getByRole("tablist", { name: "SFTP or SSH" })
    fireEvent.mouseDown(within(viewSwitcher).getByRole("tab", { name: "SSH" }))

    expect(screen.getByText("Connect a host to open a terminal session here.")).toBeInTheDocument()
    expect(screen.queryByText("Connect a host to transfer files here.")).not.toBeInTheDocument()
  })

  it("shows the agent nav and main entry states", () => {
    render(<AppShell />)
    switchMode("AgentZone")

    const nav = screen.getByTestId("nav-mode-content-agent")
    expect(within(nav).getByText("尚無 session", { selector: "p" })).toBeInTheDocument()
    expect(within(nav).getByRole("button", { name: "新增 session" })).toBeInTheDocument()
    expect(screen.getByText("ACP sessions will be managed here")).toBeInTheDocument()
  })
})

describe("Settings dialog content", () => {
  it("has the design nav sections; the language-server list no longer lives under Editor", async () => {
    render(<AppShell />)
    fireEvent.click(screen.getByRole("button", { name: "Settings" }))
    const dialog = await screen.findByRole("dialog")

    expect(within(dialog).getByRole("button", { name: "Appearance" })).toBeInTheDocument()
    expect(within(dialog).getByRole("button", { name: "Editor" })).toBeInTheDocument()
    expect(within(dialog).getByRole("button", { name: "Safety" })).toBeInTheDocument()

    fireEvent.click(within(dialog).getByRole("button", { name: "Editor" }))

    // The placeholder language-server list + fake format-on-save moved to the
    // live LSP pane (T12b-2); the editor pane keeps its own surface toggle.
    expect(within(dialog).getByRole("switch", { name: "Show minimap" })).toBeInTheDocument()
    expect(within(dialog).queryByText("TypeScript/JavaScript")).toBeNull()
    expect(within(dialog).queryByText("Not installed")).toBeNull()
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

    fireEvent.click(within(dialog).getByRole("button", { name: /^Git/ }))
    expect(within(dialog).getByText(/2\.50\.1/)).toBeInTheDocument()
    expect(within(dialog).getByRole("button", { name: "唯讀檢查" })).toHaveAttribute(
      "aria-pressed",
      "true"
    )

    fireEvent.click(within(dialog).getByRole("button", { name: "自動 fetch" }))
    expect(useGitStore.getState().remoteCheck.mode).toBe("autofetch")
  })

  it("remote-check control uses role=group (aria-pressed buttons), not radiogroup (T19)", async () => {
    useGitStore.setState({ environment: { status: "ready", root: "/w", version: "2.50.1" } })
    render(<AppShell />)
    fireEvent.click(screen.getByRole("button", { name: "Settings" }))
    const dialog = await screen.findByRole("dialog")
    fireEvent.click(within(dialog).getByRole("button", { name: /^Git/ }))

    expect(within(dialog).getByRole("group", { name: "遠端檢查" })).toBeInTheDocument()
    expect(
      within(dialog).queryByRole("radiogroup", { name: "遠端檢查" })
    ).not.toBeInTheDocument()
  })

  it("clamps the remote-check interval on blur and allows intermediate typing (T19)", async () => {
    useGitStore.setState({
      environment: { status: "ready", root: "/w", version: "2.50.1" },
      remoteCheck: { mode: "probe", intervalSec: 180 },
    })
    render(<AppShell />)
    fireEvent.click(screen.getByRole("button", { name: "Settings" }))
    const dialog = await screen.findByRole("dialog")
    fireEvent.click(within(dialog).getByRole("button", { name: /^Git/ }))

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
    expect(screen.getByText("未偵測到 Git")).toBeInTheDocument()
    expect(screen.getByRole("button", { name: "重新偵測" })).toBeInTheDocument()
  })

  it("git nav shows guided setup when git missing", () => {
    useGitStore.setState({ environment: { status: "missing", reason: "git not found" } })
    render(<GitNavContent />)
    expect(screen.getByText("未偵測到 Git")).toBeInTheDocument()
    expect(screen.getByRole("button", { name: "重新偵測" })).toBeInTheDocument()
  })
})

it("右鍵 preview 面板被完全吃掉：不彈選單、default 被擋", () => {
  render(<PreviewPanel />)
  const nativeMenuShown = fireEvent.contextMenu(screen.getByTestId("preview-panel"))
  expect(nativeMenuShown).toBe(false)
  expect(useContextMenuStore.getState().kind).toBeNull()
})

describe("PreviewPanel dev server flow", () => {
  it("does not detect or start on mount", () => {
    useWorkspaceStore.setState({ workspacePath: "/workspace" })

    render(<PreviewPanel />)

    expect(ipcMocks.devServerDetect).not.toHaveBeenCalled()
    expect(ipcMocks.devServerStart).not.toHaveBeenCalled()
  })

  it("detects candidates, starts the first dev server, and navigates to localhost", async () => {
    useWorkspaceStore.setState({ workspacePath: "/workspace" })
    ipcMocks.devServerDetect.mockResolvedValueOnce({
      candidates: [{ scriptName: "dev", command: "bun run dev", likelyPort: 5173 }],
      runningPorts: [],
    })
    ipcMocks.devServerStart.mockResolvedValueOnce({
      workspace: "/workspace",
      command: "bun run dev",
      port: 5173,
      status: { status: "running", port: 5173 },
    })

    render(<PreviewPanel />)
    fireEvent.click(screen.getByRole("button", { name: "啟動 dev server" }))

    await waitFor(() =>
      expect(ipcMocks.devServerStart).toHaveBeenCalledWith(
        "/workspace",
        "bun run dev",
        5173,
        expect.any(Function)
      )
    )
    expect((screen.getByLabelText("Preview URL") as HTMLInputElement).value).toBe(
      "http://localhost:5173"
    )
    expect(usePreviewStore.getState().navForWorkspace("/workspace").url).toBe(
      "http://localhost:5173"
    )
  })

  it("uses persisted preview command and port overrides when starting", async () => {
    localStorage.setItem(
      "yuzora:preview-settings",
      JSON.stringify({ command: "bun run preview:custom", port: "6000" })
    )
    useWorkspaceStore.setState({ workspacePath: "/workspace" })
    ipcMocks.devServerDetect.mockResolvedValueOnce({
      candidates: [{ scriptName: "dev", command: "bun run dev", likelyPort: 5173 }],
      runningPorts: [],
    })
    ipcMocks.devServerStart.mockResolvedValueOnce({
      workspace: "/workspace",
      command: "bun run preview:custom",
      port: 6000,
      status: { status: "running", port: 6000 },
    })

    render(<PreviewPanel />)
    fireEvent.click(screen.getByRole("button", { name: "啟動 dev server" }))

    await waitFor(() => expect(ipcMocks.devServerDetect).toHaveBeenCalledWith("/workspace", [6000]))
    await waitFor(() =>
      expect(ipcMocks.devServerStart).toHaveBeenCalledWith(
        "/workspace",
        "bun run preview:custom",
        6000,
        expect.any(Function)
      )
    )
    expect(usePreviewStore.getState().navForWorkspace("/workspace").url).toBe(
      "http://localhost:6000"
    )
  })

  it("triggers the occupied-port flow when the persisted override port is running", async () => {
    localStorage.setItem(
      "yuzora:preview-settings",
      JSON.stringify({ command: "", port: "6000" })
    )
    useWorkspaceStore.setState({ workspacePath: "/workspace" })
    ipcMocks.devServerDetect.mockResolvedValueOnce({
      candidates: [{ scriptName: "dev", command: "bun run dev", likelyPort: 5173 }],
      runningPorts: [6000],
    })

    render(<PreviewPanel />)
    fireEvent.click(screen.getByRole("button", { name: "啟動 dev server" }))

    expect(await screen.findByRole("button", { name: /連接現有 server.*6000/ })).toBeInTheDocument()
    expect(ipcMocks.devServerDetect).toHaveBeenCalledWith("/workspace", [6000])
    expect(ipcMocks.devServerStart).not.toHaveBeenCalled()
  })

  it("uses the backend-confirmed port when it differs from the persisted override", async () => {
    localStorage.setItem(
      "yuzora:preview-settings",
      JSON.stringify({ command: "bun run preview:custom", port: "6000" })
    )
    useWorkspaceStore.setState({ workspacePath: "/workspace" })
    ipcMocks.devServerDetect.mockResolvedValueOnce({
      candidates: [{ scriptName: "dev", command: "bun run dev", likelyPort: 5173 }],
      runningPorts: [],
    })
    ipcMocks.devServerStart.mockResolvedValueOnce({
      workspace: "/workspace",
      command: "bun run preview:custom",
      port: 6000,
      status: { status: "running", port: 6000 },
    })

    render(<PreviewPanel />)
    fireEvent.click(screen.getByRole("button", { name: "啟動 dev server" }))

    await waitFor(() =>
      expect((screen.getByLabelText("Preview URL") as HTMLInputElement).value).toBe(
        "http://localhost:6000"
      )
    )

    act(() => {
      usePreviewStore.getState().setDevServer({
        workspace: "/workspace",
        command: "bun run preview:custom",
        port: 6000,
        status: { status: "running", port: 6001 },
      })
    })

    await waitFor(() =>
      expect((screen.getByLabelText("Preview URL") as HTMLInputElement).value).toBe(
        "http://localhost:6001"
      )
    )
    expect(usePreviewStore.getState().navForWorkspace("/workspace").url).toBe(
      "http://localhost:6001"
    )
  })

  it("keeps the current preview path when backend status confirms the same port", () => {
    useWorkspaceStore.setState({ workspacePath: "/workspace" })
    usePreviewStore.getState().navigate("/workspace", "http://localhost:5173/about")
    usePreviewStore.getState().setDevServer({
      workspace: "/workspace",
      command: "bun run dev",
      port: 5173,
      status: { status: "running", port: 5173 },
    })

    render(<PreviewPanel />)

    expect((screen.getByLabelText("Preview URL") as HTMLInputElement).value).toBe(
      "http://localhost:5173/about"
    )
  })

  it("ignores invalid persisted override ports with a visible hint", async () => {
    localStorage.setItem(
      "yuzora:preview-settings",
      JSON.stringify({ command: "", port: "6000abc" })
    )
    useWorkspaceStore.setState({ workspacePath: "/workspace" })
    ipcMocks.devServerDetect.mockResolvedValueOnce({
      candidates: [{ scriptName: "dev", command: "bun run dev", likelyPort: 5173 }],
      runningPorts: [],
    })
    ipcMocks.devServerStart.mockResolvedValueOnce({
      workspace: "/workspace",
      command: "bun run dev",
      port: 5173,
      status: { status: "running", port: 5173 },
    })

    render(<PreviewPanel />)
    fireEvent.click(screen.getByRole("button", { name: "啟動 dev server" }))

    expect(await screen.findByRole("status")).toHaveTextContent("6000abc")
    expect(ipcMocks.devServerDetect).toHaveBeenCalledWith("/workspace")
    await waitFor(() =>
      expect(ipcMocks.devServerStart).toHaveBeenCalledWith(
        "/workspace",
        "bun run dev",
        5173,
        expect.any(Function)
      )
    )
  })

  it("drops pending detect results after switching workspace", async () => {
    const detect = deferred<{
      candidates: Array<{ scriptName: string; command: string; likelyPort: number | null }>
      runningPorts: number[]
    }>()
    useWorkspaceStore.setState({ workspacePath: "/workspace-a" })
    ipcMocks.devServerDetect.mockReturnValueOnce(detect.promise)

    render(<PreviewPanel />)
    fireEvent.click(screen.getByRole("button", { name: "啟動 dev server" }))
    expect(ipcMocks.devServerDetect).toHaveBeenCalledWith("/workspace-a")

    act(() => {
      useWorkspaceStore.setState({ workspacePath: "/workspace-b" })
    })
    await act(async () => {
      detect.resolve({
        candidates: [{ scriptName: "dev", command: "bun run dev", likelyPort: 5173 }],
        runningPorts: [],
      })
      await detect.promise
    })

    expect(ipcMocks.devServerStart).not.toHaveBeenCalled()
    expect(usePreviewStore.getState().devServerForWorkspace("/workspace-a")).toBeNull()
    expect(usePreviewStore.getState().devServerForWorkspace("/workspace-b")).toBeNull()
    expect(usePreviewStore.getState().navForWorkspace("/workspace-a").url).toBeNull()
    expect(usePreviewStore.getState().navForWorkspace("/workspace-b").url).toBeNull()
  })

  it("stops a stale started server after switching workspace", async () => {
    const start = deferred<{
      workspace: string
      command: string
      port: number
      status: { status: "running"; port: number }
    }>()
    useWorkspaceStore.setState({ workspacePath: "/workspace-a" })
    ipcMocks.devServerDetect.mockResolvedValueOnce({
      candidates: [{ scriptName: "dev", command: "bun run dev", likelyPort: 5173 }],
      runningPorts: [],
    })
    ipcMocks.devServerStart.mockReturnValueOnce(start.promise)
    ipcMocks.devServerStop.mockResolvedValueOnce(undefined)

    render(<PreviewPanel />)
    fireEvent.click(screen.getByRole("button", { name: "啟動 dev server" }))

    await waitFor(() => expect(ipcMocks.devServerStart).toHaveBeenCalled())
    act(() => {
      useWorkspaceStore.setState({ workspacePath: "/workspace-b" })
    })
    await act(async () => {
      start.resolve({
        workspace: "/workspace-a",
        command: "bun run dev",
        port: 5173,
        status: { status: "running", port: 5173 },
      })
      await start.promise
    })

    await waitFor(() => expect(ipcMocks.devServerStop).toHaveBeenCalledWith("/workspace-a"))
    expect(usePreviewStore.getState().navForWorkspace("/workspace-a").url).toBeNull()
    expect(usePreviewStore.getState().navForWorkspace("/workspace-b").url).toBeNull()
    expect(usePreviewStore.getState().devServerForWorkspace("/workspace-b")).toBeNull()
  })

  it("starts normally when another port is occupied but the candidate port is free", async () => {
    useWorkspaceStore.setState({ workspacePath: "/workspace" })
    ipcMocks.devServerDetect.mockResolvedValueOnce({
      candidates: [{ scriptName: "dev", command: "bun run dev", likelyPort: 5173 }],
      runningPorts: [3000],
    })
    ipcMocks.devServerStart.mockResolvedValueOnce({
      workspace: "/workspace",
      command: "bun run dev",
      port: 5173,
      status: { status: "running", port: 5173 },
    })

    render(<PreviewPanel />)
    fireEvent.click(screen.getByRole("button", { name: "啟動 dev server" }))

    await waitFor(() =>
      expect(ipcMocks.devServerStart).toHaveBeenCalledWith(
        "/workspace",
        "bun run dev",
        5173,
        expect.any(Function)
      )
    )
    expect(screen.queryByRole("button", { name: /連接現有 server/ })).not.toBeInTheDocument()
  })

  it("shows port-occupied choices and connects to an existing server without spawning", async () => {
    useWorkspaceStore.setState({ workspacePath: "/workspace" })
    ipcMocks.devServerDetect.mockResolvedValueOnce({
      candidates: [{ scriptName: "dev", command: "bun run dev", likelyPort: 5173 }],
      runningPorts: [3000, 5173],
    })

    render(<PreviewPanel />)
    fireEvent.click(screen.getByRole("button", { name: "啟動 dev server" }))

    expect(await screen.findByRole("button", { name: /連接現有 server.*5173/ })).toBeInTheDocument()
    expect(screen.getByRole("button", { name: "切換 port 後啟動" })).toBeInTheDocument()
    expect(screen.getByRole("button", { name: "重新偵測" })).toBeInTheDocument()

    fireEvent.click(screen.getByRole("button", { name: /連接現有 server.*5173/ }))

    expect(ipcMocks.devServerStart).not.toHaveBeenCalled()
    expect((screen.getByLabelText("Preview URL") as HTMLInputElement).value).toBe(
      "http://localhost:5173"
    )
  })

  it("starts a candidate on a changed port from the occupied-port flow", async () => {
    useWorkspaceStore.setState({ workspacePath: "/workspace" })
    ipcMocks.devServerDetect.mockResolvedValueOnce({
      candidates: [{ scriptName: "dev", command: "bun run dev", likelyPort: 5173 }],
      runningPorts: [5173],
    })
    ipcMocks.devServerStart.mockResolvedValueOnce({
      workspace: "/workspace",
      command: "bun run dev",
      port: 6000,
      status: { status: "running", port: 6000 },
    })

    render(<PreviewPanel />)
    fireEvent.click(screen.getByRole("button", { name: "啟動 dev server" }))
    const port = await screen.findByRole("spinbutton", { name: "替代 port" })
    fireEvent.change(port, { target: { value: "6000" } })
    fireEvent.click(screen.getByRole("button", { name: "切換 port 後啟動" }))

    await waitFor(() =>
      expect(ipcMocks.devServerStart).toHaveBeenCalledWith(
        "/workspace",
        "bun run dev",
        6000,
        expect.any(Function)
      )
    )
  })

  it("shows failed status with retry action", async () => {
    useWorkspaceStore.setState({ workspacePath: "/workspace" })
    usePreviewStore.getState().setDevServer({
      workspace: "/workspace",
      command: "bun run dev",
      port: 5173,
      status: { status: "failed", reason: "command not found" },
    })
    ipcMocks.devServerDetect.mockResolvedValueOnce({
      candidates: [{ scriptName: "dev", command: "bun run dev", likelyPort: 5173 }],
      runningPorts: [],
    })
    ipcMocks.devServerStart.mockResolvedValueOnce({
      workspace: "/workspace",
      command: "bun run dev",
      port: 5173,
      status: { status: "running", port: 5173 },
    })

    render(<PreviewPanel />)

    expect(screen.getByText("command not found")).toBeInTheDocument()
    fireEvent.click(screen.getByRole("button", { name: "重試啟動" }))

    await waitFor(() => expect(ipcMocks.devServerStart).toHaveBeenCalled())
  })

  it("opens Logs with the dev_server source from a failed preview and renders matching rows", async () => {
    logMocks.queryResult = previewLogRows
    useWorkspaceStore.setState({ workspacePath: "/workspace" })
    usePreviewStore.getState().setDevServer({
      workspace: "/workspace",
      command: "bun run dev",
      port: 5173,
      status: { status: "failed", reason: "command not found" },
    })

    render(<PreviewWithSettings />)
    fireEvent.click(screen.getByRole("button", { name: "檢視 logs" }))
    const dialog = await screen.findByRole("dialog")

    expect(await within(dialog).findByRole("heading", { name: "Logs" })).toBeInTheDocument()
    await waitFor(() =>
      expect(logMocks.logQueryCalls.at(-1)).toEqual({
        filters: {
          sources: ["dev_server"],
          limit: 500,
        },
      }),
    )
    expect(await within(dialog).findByTestId("log-row-dev_server_start_failed")).toBeInTheDocument()
  })

  it("stops a running server and leaves the preview restartable", async () => {
    useWorkspaceStore.setState({ workspacePath: "/workspace" })
    usePreviewStore.getState().setDevServer({
      workspace: "/workspace",
      command: "bun run dev",
      port: 5173,
      status: { status: "running", port: 5173 },
    })
    ipcMocks.devServerStop.mockResolvedValueOnce(undefined)

    render(<PreviewPanel />)
    fireEvent.click(screen.getByRole("button", { name: "Stop" }))

    await waitFor(() => expect(ipcMocks.devServerStop).toHaveBeenCalledWith("/workspace"))
    expect(screen.getByText("Exited")).toBeInTheDocument()
    expect(screen.getByRole("button", { name: "啟動 dev server" })).toBeInTheDocument()
  })

  it("wires back, forward, reload, and responsive frame controls to previewStore", () => {
    useWorkspaceStore.setState({ workspacePath: "/workspace" })
    const store = usePreviewStore.getState()
    store.navigate("/workspace", "http://localhost:5173")
    store.navigate("/workspace", "http://localhost:5173/about")

    render(<PreviewPanel />)

    fireEvent.click(screen.getByRole("button", { name: "Back" }))
    expect(usePreviewStore.getState().navForWorkspace("/workspace").url).toBe(
      "http://localhost:5173"
    )

    fireEvent.click(screen.getByRole("button", { name: "Forward" }))
    expect(usePreviewStore.getState().navForWorkspace("/workspace").url).toBe(
      "http://localhost:5173/about"
    )

    fireEvent.click(screen.getByRole("button", { name: "Reload" }))
    expect(usePreviewStore.getState().navForWorkspace("/workspace").reloadNonce).toBe(1)

    fireEvent.click(screen.getByRole("button", { name: "Toggle responsive frame" }))
    expect(usePreviewStore.getState().navForWorkspace("/workspace").frame).toBe("mobile")
    expect(screen.getByTestId("preview-frame-shell")).toHaveStyle({ width: "390px" })
  })
})

it("右鍵 Git 面板開啟 git 選單", () => {
  render(<GitPanel />)
  // Default tab is Log; its details panel always shows this prompt.
  fireEvent.contextMenu(screen.getByText("Select a commit to view details"))
  expect(useContextMenuStore.getState().kind).toBe("git")
})

it("右鍵 Agent 面板開啟 agent 選單", () => {
  render(<AgentZonePanel />)
  fireEvent.contextMenu(screen.getByText("ACP sessions will be managed here"))
  expect(useContextMenuStore.getState().kind).toBe("agent")
})

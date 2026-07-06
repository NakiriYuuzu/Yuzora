import { beforeEach, describe, expect, it, vi } from "vitest"
import { act, cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react"
import { clearMocks, mockIPC } from "@tauri-apps/api/mocks"

import { SettingsDialog } from "@/app/workbench/SettingsDialog"
import { useLspStore } from "@/state/lspStore"
import { uiInitialState, useUiStore } from "@/state/uiStore"
import { useWorkspaceStore } from "@/state/workspaceStore"

const writeText = vi.fn(async (_value: string) => undefined)
const save = vi.fn(async (_options: unknown) => "/tmp/yuzora-logs.zip")
const openPath = vi.fn(async (_path: string) => undefined)
const homeDir = vi.fn(async () => "/Users/tester")
const join = vi.fn(async (...parts: string[]) => parts.join("/"))

vi.mock("@tauri-apps/plugin-clipboard-manager", () => ({
  writeText: (value: string) => writeText(value),
}))

vi.mock("@tauri-apps/plugin-dialog", () => ({
  save: (options: unknown) => save(options),
}))

vi.mock("@tauri-apps/plugin-opener", () => ({
  openPath: (path: string) => openPath(path),
}))

vi.mock("@tauri-apps/api/path", () => ({
  homeDir: () => homeDir(),
  join: (...parts: string[]) => join(...parts),
}))

vi.mock("@tauri-apps/api/event", () => ({
  listen: vi.fn(async () => () => {}),
}))

const logRows = [
  {
    timestamp: "2026-01-02T03:04:05+08:00",
    level: "error",
    kind: "audit",
    source: "lsp",
    workspace_path: "/ws",
    event: "lsp_restart",
    message: "server crashed",
    metadata: { language: "typescript", restartCount: 2 },
  },
]

const devServerLogRows = [
  {
    timestamp: "2026-01-02T04:05:06+08:00",
    level: "error",
    kind: "debug",
    source: "dev_server",
    workspace_path: "/ws",
    event: "dev_server_start_failed",
    message: "command not found",
    metadata: { command: "bun run dev" },
  },
]

let logQueryCalls: Array<Record<string, unknown>> = []
let queryResult: unknown[] = []
let sourcesResult: string[] = []

function setupIpc() {
  mockIPC((cmd, args) => {
    const a = (args ?? {}) as Record<string, unknown>
    if (cmd === "log_sources") return sourcesResult
    if (cmd === "log_query") {
      logQueryCalls.push((args ?? {}) as Record<string, unknown>)
      return queryResult
    }
    if (cmd === "lsp_config_get") return { defaults: {}, workspaces: {} }
    if (cmd === "lsp_config_stale") return []
    if (cmd === "lsp_status") return []
    if (cmd === "lsp_set_trace") return undefined
    if (cmd === "lsp_config_set_server") return { defaults: { [a.language as string]: a.serverId }, workspaces: {} }
    return undefined
  })
}

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

function renderDialog(props: Partial<React.ComponentProps<typeof SettingsDialog>> = {}) {
  return render(
    <SettingsDialog
      open
      onOpenChange={() => {}}
      theme="light"
      onThemeChange={() => {}}
      initialSection="logs"
      {...props}
    />,
  )
}

function SettingsHarness() {
  const open = useUiStore((s) => s.settingsOpen)
  const setOpen = useUiStore((s) => s.setSettingsOpen)
  const section = useUiStore((s) => s.settingsSection)
  const language = useUiStore((s) => s.settingsLanguage)
  const nonce = useUiStore((s) => s.settingsNonce)

  return (
    <SettingsDialog
      open={open}
      onOpenChange={setOpen}
      theme="light"
      onThemeChange={() => {}}
      initialSection={section ?? undefined}
      initialLanguage={language ?? undefined}
      openNonce={nonce}
    />
  )
}

beforeEach(() => {
  installLocalStorage()
  cleanup()
  clearMocks()
  vi.clearAllMocks()
  logQueryCalls = []
  queryResult = []
  sourcesResult = ["ui", "lsp", "agent", "dev_server"]
  useUiStore.setState(uiInitialState)
  useLspStore.getState().reset()
  useWorkspaceStore.setState({ workspacePath: "/ws" })
  setupIpc()
})

describe("SettingsDialog logs section", () => {
  it("builds log_query filters from kind, level, source, text, and time range", async () => {
    renderDialog()

    expect(await screen.findByRole("heading", { name: "Logs" })).toBeInTheDocument()
    await waitFor(() => expect(logQueryCalls.length).toBeGreaterThan(0))

    fireEvent.click(
      within(screen.getByRole("group", { name: "kind 篩選" })).getByRole("button", {
        name: "audit",
      }),
    )
    fireEvent.click(
      within(screen.getByRole("group", { name: "level 篩選" })).getByRole("button", {
        name: "error",
      }),
    )
    fireEvent.change(screen.getByRole("combobox", { name: "source 篩選" }), {
      target: { value: "lsp" },
    })
    fireEvent.change(screen.getByRole("searchbox", { name: "文字搜尋" }), {
      target: { value: "server" },
    })
    fireEvent.change(screen.getByLabelText("since"), {
      target: { value: "2026-01-02T00:00:00+08:00" },
    })
    fireEvent.change(screen.getByLabelText("until"), {
      target: { value: "2026-01-03T00:00:00+08:00" },
    })

    await waitFor(() =>
      expect(logQueryCalls.at(-1)).toEqual({
        filters: {
          since: "2026-01-02T00:00:00+08:00",
          until: "2026-01-03T00:00:00+08:00",
          levels: ["error"],
          kinds: ["audit"],
          sources: ["lsp"],
          text: "server",
          limit: 500,
        },
      }),
    )
  })

  it("applies an initial source target to log_query and renders matching rows", async () => {
    queryResult = devServerLogRows
    useUiStore.setState({ settingsLogSource: "dev_server", settingsNonce: 1 })

    renderDialog({ openNonce: 1 })

    expect(await screen.findByRole("heading", { name: "Logs" })).toBeInTheDocument()
    await waitFor(() =>
      expect(logQueryCalls.at(-1)).toEqual({
        filters: {
          sources: ["dev_server"],
          limit: 500,
        },
      }),
    )
    expect(await screen.findByTestId("log-row-dev_server_start_failed")).toBeInTheDocument()
  })

  it("opens Logs with the lsp source from a failed LSP card and renders matching rows", async () => {
    queryResult = logRows
    useLspStore.getState().setServerInfo({
      workspace: "/ws",
      language: "python",
      serverId: "pylsp",
      command: "uv run pylsp",
      path: "/bin/pylsp",
      status: { status: "crashed", reason: "boom" },
      lastStartupLog: null,
      lastError: "spawn pylsp failed",
      restartCount: 1,
    })

    render(<SettingsHarness />)
    act(() => useUiStore.getState().openSettings("lsp", "python"))
    const dialog = await screen.findByRole("dialog")
    const card = await within(dialog).findByTestId("lsp-card-python")
    expect(within(card).getByText("spawn pylsp failed")).toBeInTheDocument()

    fireEvent.click(within(card).getByRole("button", { name: "檢視 logs" }))

    expect(await within(dialog).findByRole("heading", { name: "Logs" })).toBeInTheDocument()
    await waitFor(() =>
      expect(logQueryCalls.at(-1)).toEqual({
        filters: {
          sources: ["lsp"],
          limit: 500,
        },
      }),
    )
    expect(await within(dialog).findByTestId("log-row-lsp_restart")).toBeInTheDocument()
  })

  it("renders timestamp, level, kind, source, event, and message columns", async () => {
    queryResult = logRows
    renderDialog()

    const row = await screen.findByTestId("log-row-lsp_restart")

    expect(within(row).getByText("2026-01-02T03:04:05+08:00")).toBeInTheDocument()
    expect(within(row).getByText("error")).toBeInTheDocument()
    expect(within(row).getByText("audit")).toBeInTheDocument()
    expect(within(row).getByText("lsp")).toBeInTheDocument()
    expect(within(row).getByText("lsp_restart")).toBeInTheDocument()
    expect(within(row).getByText("server crashed")).toBeInTheDocument()
  })

  it("expands a row to show metadata JSON", async () => {
    queryResult = logRows
    renderDialog()

    fireEvent.click(await screen.findByRole("button", { name: "展開 metadata lsp_restart" }))

    expect(await screen.findByText(/"language": "typescript"/)).toBeInTheDocument()
    expect(screen.getByText(/"restartCount": 2/)).toBeInTheDocument()
  })
})

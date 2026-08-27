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
  it("rejects an invalid ISO timestamp accessibly without issuing a query", async () => {
    renderDialog()
    await waitFor(() => expect(logQueryCalls.length).toBeGreaterThan(0))
    await act(async () => {
      await new Promise((resolve) => setTimeout(resolve, 350))
    })
    logQueryCalls = []
    const since = screen.getByLabelText("since")

    fireEvent.change(since, { target: { value: "not-a-date" } })

    expect(since).toHaveAttribute("aria-invalid", "true")
    expect(await screen.findByRole("alert")).toHaveTextContent(
      "since must be a valid ISO 8601 timestamp.",
    )
    await act(async () => {
      await new Promise((resolve) => setTimeout(resolve, 350))
    })
    expect(logQueryCalls).toEqual([])
  })

  it("accepts backend-supported date-only and datetime-local time filters", async () => {
    renderDialog()
    await waitFor(() => expect(logQueryCalls.length).toBeGreaterThan(0))
    logQueryCalls = []

    const since = screen.getByLabelText("since")
    const until = screen.getByLabelText("until")
    fireEvent.change(since, { target: { value: "2026-01-02" } })
    fireEvent.change(until, { target: { value: "2026-01-03T12:34" } })

    expect(since).not.toHaveAttribute("aria-invalid", "true")
    expect(until).not.toHaveAttribute("aria-invalid", "true")
    await waitFor(() =>
      expect(logQueryCalls.at(-1)).toEqual({
        filters: {
          since: "2026-01-02",
          until: "2026-01-03T12:34",
          limit: 500,
        },
      }),
    )
  })

  it("rejects calendar-invalid date-only and datetime-local time filters", async () => {
    renderDialog()
    await waitFor(() => expect(logQueryCalls.length).toBeGreaterThan(0))
    logQueryCalls = []

    const since = screen.getByLabelText("since")
    const until = screen.getByLabelText("until")
    fireEvent.change(since, { target: { value: "2026-02-30" } })
    fireEvent.change(until, { target: { value: "2026-01-03T24:00" } })

    expect(since).toHaveAttribute("aria-invalid", "true")
    expect(until).toHaveAttribute("aria-invalid", "true")
    expect(screen.getByText("since must be a valid ISO 8601 timestamp.")).toBeInTheDocument()
    expect(screen.getByText("until must be a valid ISO 8601 timestamp.")).toBeInTheDocument()
    await act(async () => {
      await new Promise((resolve) => setTimeout(resolve, 350))
    })
    expect(logQueryCalls).toEqual([])
  })

  it("paginates a 500-row result into an accessible bounded list", async () => {
    queryResult = Array.from({ length: 500 }, (_, index) => ({
      ...logRows[0],
      timestamp: `2026-01-02T03:${String(index % 60).padStart(2, "0")}:05+08:00`,
      event: `event_${index}`,
    }))
    renderDialog()

    expect(await screen.findByText("Page 1 of 10")).toBeInTheDocument()
    const list = screen.getByRole("list", { name: "Log results" })
    expect(within(list).getAllByRole("listitem")).toHaveLength(50)
    expect(screen.getByRole("button", { name: "Previous results page" })).toHaveAttribute(
      "data-slot",
      "button",
    )
    expect(screen.getByRole("button", { name: "Next results page" })).toHaveAttribute(
      "data-slot",
      "button",
    )
    expect(screen.getByRole("button", { name: "Next results page" })).toBeEnabled()
  })

  it("does not carry expanded metadata to a different row at the same page-relative index", async () => {
    queryResult = Array.from({ length: 51 }, (_, index) => ({
      ...logRows[0],
      timestamp:
        index === 0 || index === 50
          ? "2026-01-02T03:04:05+08:00"
          : `2026-01-02T03:${String(index).padStart(2, "0")}:05+08:00`,
      event: index === 0 || index === 50 ? "repeated_event" : `event_${index}`,
      metadata: { pageIndex: index },
    }))
    renderDialog()

    expect(await screen.findByText("Page 1 of 2")).toBeInTheDocument()
    fireEvent.click(screen.getByRole("button", { name: "Expand metadata repeated_event" }))
    expect(await screen.findByText(/"pageIndex": 0/)).toBeInTheDocument()

    fireEvent.click(screen.getByRole("button", { name: "Next results page" }))

    expect(await screen.findByText("Page 2 of 2")).toBeInTheDocument()
    expect(
      screen.getByRole("button", { name: "Expand metadata repeated_event" }),
    ).toBeInTheDocument()
    expect(screen.queryByText(/"pageIndex": 50/)).not.toBeInTheDocument()
  })

  it("renders the English locale without hard-coded Chinese product strings", async () => {
    renderDialog()
    await screen.findByRole("heading", { name: "Logs" })
    await waitFor(() => expect(logQueryCalls.length).toBeGreaterThan(0))

    expect(screen.getByRole("dialog").textContent).not.toMatch(/\p{Script=Han}/u)
  })

  it("builds log_query filters from kind, level, source, text, and time range", async () => {
    renderDialog()

    expect(await screen.findByRole("heading", { name: "Logs" })).toBeInTheDocument()
    await waitFor(() => expect(logQueryCalls.length).toBeGreaterThan(0))

    fireEvent.click(
      within(screen.getByRole("group", { name: "kind filter" })).getByRole("button", {
        name: "audit",
      }),
    )
    fireEvent.click(
      within(screen.getByRole("group", { name: "level filter" })).getByRole("button", {
        name: "error",
      }),
    )
    fireEvent.change(screen.getByRole("combobox", { name: "source filter" }), {
      target: { value: "lsp" },
    })
    fireEvent.change(screen.getByRole("searchbox", { name: "Text search" }), {
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

  it("debounces rapid text input into a single log_query", async () => {
    renderDialog()

    expect(await screen.findByRole("heading", { name: "Logs" })).toBeInTheDocument()
    await waitFor(() => expect(logQueryCalls.length).toBeGreaterThan(0))
    const baseline = logQueryCalls.length

    const searchbox = screen.getByRole("searchbox", { name: "Text search" })
    fireEvent.change(searchbox, { target: { value: "s" } })
    fireEvent.change(searchbox, { target: { value: "se" } })
    fireEvent.change(searchbox, { target: { value: "ser" } })
    fireEvent.change(searchbox, { target: { value: "serv" } })

    await waitFor(() => expect(logQueryCalls.length).toBe(baseline + 1))
    expect(logQueryCalls.at(-1)).toMatchObject({ filters: { text: "serv", limit: 500 } })
  })

  it("delays the query by the debounce window before rendering rows", async () => {
    queryResult = logRows
    renderDialog()

    // debounced：mount 後同步當下尚未觸發 log_query
    expect(logQueryCalls.length).toBe(0)
    expect(await screen.findByTestId("log-row-lsp_restart")).toBeInTheDocument()
    expect(logQueryCalls.length).toBeGreaterThan(0)
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

    fireEvent.click(within(card).getByRole("button", { name: "View logs" }))

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

    fireEvent.click(await screen.findByRole("button", { name: "Expand metadata lsp_restart" }))

    expect(await screen.findByText(/"language": "typescript"/)).toBeInTheDocument()
    expect(screen.getByText(/"restartCount": 2/)).toBeInTheDocument()
  })
})

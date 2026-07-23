import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"
import { act, cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react"
import { clearMocks, mockIPC } from "@tauri-apps/api/mocks"

import { AppShell } from "@/app/AppShell"
import {
  PREVIEW_SETTINGS_STORAGE_KEY,
  SettingsDialog,
  TERMINAL_SETTINGS_STORAGE_KEY,
} from "@/app/workbench/SettingsDialog"
import { loadTerminalSettings } from "@/app/workbench/settingsStorage"
import { FORMAT_ON_SAVE_STORAGE_KEY } from "@/editor/EditorPane"
import type { LspConfig, LspServerInfo } from "@/lib/types"
import { useLspStore } from "@/state/lspStore"
import { uiInitialState, useUiStore } from "@/state/uiStore"
import {
  WORKBENCH_LAYOUT_STORAGE_KEY,
  useWorkbenchLayoutStore,
  workbenchLayoutInitialState,
} from "@/state/workbenchLayoutStore"
import { useWorkspaceStore } from "@/state/workspaceStore"

// Capture event listeners so tests can emit lsp:install-progress (same shape as
// LspBridge.test — the Rust side pushes install phases over this channel).
const listeners = new Map<string, (e: unknown) => void>()
// The unlisten fn each listen() returns, spied so a test can assert cleanup runs.
const unlistenSpies = new Map<string, ReturnType<typeof vi.fn>>()
vi.mock("@tauri-apps/api/event", () => ({
  listen: vi.fn(async (event: string, cb: unknown) => {
    listeners.set(event, cb as (e: unknown) => void)
    const spy = vi.fn(() => void listeners.delete(event))
    unlistenSpies.set(event, spy)
    return spy
  }),
}))

// Recorded IPC arguments + configurable responses, reset per test.
let traceCalls: boolean[] = []
// Full lsp_install_server args so a test can assert the workspace override is
// forwarded (W6A-F1) — raw a.workspace (undefined when the old code omitted it).
let installCalls: Array<{ workspace: string | null; language: string }> = []
let detectCalls: Array<{ workspace: string | null; language: string }> = []
let detectHandler: (
  workspace: string | null,
  language: string,
) => LspServerInfo | Promise<LspServerInfo>
let clearStaleCalls: string[] = []
let setServerCalls: Array<{ workspace: string | null; language: string; serverId: string }> = []
let staleResult: string[] = []
let configResult: LspConfig = { defaults: {}, workspaces: {} }
let installReject: string | null = null
let traceReject = false
// When set, each lsp_set_trace call hangs until the test settles it — lets a test
// interleave a stale (superseded) reject with newer toggles (R2A-F1 gen guard).
let traceDeferred = false
const traceResolvers: Array<{ resolve: () => void; reject: (e: unknown) => void }> = []
// When set, lsp_install_server hangs until the test resolves/rejects it — lets a
// test interleave a late terminal progress event with the install promise (A-F1).
let useInstallDeferred = false
let installResolvers: { resolve: (v: LspServerInfo) => void; reject: (e: unknown) => void } | null =
  null

// The Bun-hosted test runtime injects an empty `localStorage` global with no
// Storage methods, which also shadows jsdom's implementation. Install a minimal
// in-memory Storage so the format-on-save persistence is exercised for real.
// (Same helper as gitStore.test / editorPane.lsp.test; the proper home is
// src/test/setup.ts, outside this task's file scope — see the report hand-off.)
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

function serverInfo(language: string, over: Partial<LspServerInfo> = {}): LspServerInfo {
  return {
    workspace: "/ws",
    language,
    serverId: `${language}-srv`,
    command: "cmd",
    path: "/bin/cmd",
    status: { status: "starting" },
    lastStartupLog: null,
    lastError: null,
    restartCount: 0,
    ...over,
  }
}

function setupIpc() {
  mockIPC((cmd, args) => {
    const a = (args ?? {}) as Record<string, unknown>
    switch (cmd) {
      case "lsp_config_get":
        return configResult
      case "lsp_config_stale":
        return staleResult
      case "lsp_config_clear_stale":
        clearStaleCalls.push(a.workspace as string)
        return configResult
      case "lsp_config_set_server":
        setServerCalls.push(a as { workspace: string | null; language: string; serverId: string })
        return configResult
      case "lsp_set_trace":
        traceCalls.push(a.enabled as boolean)
        if (traceReject) return Promise.reject("trace failed")
        if (traceDeferred) {
          return new Promise<void>((resolve, reject) => {
            traceResolvers.push({ resolve, reject })
          })
        }
        return undefined
      case "lsp_install_server":
        installCalls.push({ workspace: a.workspace as string | null, language: a.language as string })
        if (installReject) return Promise.reject(installReject)
        if (useInstallDeferred) {
          return new Promise<LspServerInfo>((resolve, reject) => {
            installResolvers = { resolve, reject }
          })
        }
        return serverInfo(a.language as string, { status: { status: "starting" } })
      case "lsp_detect_server": {
        const request = {
          workspace: a.workspace as string | null,
          language: a.language as string,
        }
        detectCalls.push(request)
        return detectHandler(request.workspace, request.language)
      }
      case "lsp_status":
        return []
      default:
        // log_event + any unrelated AppShell mount IPC resolve to undefined.
        return undefined
    }
  })
}

function renderDialog(props: Partial<React.ComponentProps<typeof SettingsDialog>> = {}) {
  return render(
    <SettingsDialog
      open
      onOpenChange={() => {}}
      theme="light"
      onThemeChange={() => {}}
      {...props}
    />,
  )
}

beforeEach(() => {
  installLocalStorage()
  traceCalls = []
  installCalls = []
  detectCalls = []
  detectHandler = (workspace, language) =>
    serverInfo(language, {
      workspace: workspace ?? "",
      path: null,
      status: { status: "missing", installHint: "install" },
    })
  clearStaleCalls = []
  setServerCalls = []
  staleResult = []
  configResult = { defaults: {}, workspaces: {} }
  installReject = null
  traceReject = false
  traceDeferred = false
  traceResolvers.length = 0
  useInstallDeferred = false
  installResolvers = null
  listeners.clear()
  unlistenSpies.clear()
  useLspStore.getState().reset()
  useWorkspaceStore.setState({ workspacePath: "/ws" })
  useUiStore.setState(uiInitialState)
  localStorage.clear()
  useWorkbenchLayoutStore.setState({
    ...workbenchLayoutInitialState,
    terminalWorkspaceRatios: {},
  })
  setupIpc()
})

afterEach(() => {
  cleanup()
  clearMocks()
  vi.clearAllMocks()
})

describe("SettingsDialog LSP section", () => {
  it("renders the LSP section with all four language cards", async () => {
    renderDialog({ initialSection: "lsp" })

    expect(await screen.findByTestId("lsp-card-typescript")).toBeInTheDocument()
    expect(screen.getByTestId("lsp-card-python")).toBeInTheDocument()
    expect(screen.getByTestId("lsp-card-rust")).toBeInTheDocument()
    expect(screen.getByTestId("lsp-card-markdown")).toBeInTheDocument()
    expect(screen.getByRole("heading", { name: "LSP" })).toBeInTheDocument()
  })

  it("detects an installed-but-not-started server on mount and hides install", async () => {
    detectHandler = (workspace, language) =>
      serverInfo(language, {
        workspace: workspace ?? "",
        path: language === "python" ? "/bin/pyright" : null,
        status:
          language === "python"
            ? { status: "stopped" }
            : { status: "missing", installHint: "install" },
      })

    renderDialog({ initialSection: "lsp" })
    const card = await screen.findByTestId("lsp-card-python")

    expect(await within(card).findByText("已安裝")).toBeInTheDocument()
    expect(within(card).queryByRole("button", { name: "一鍵安裝" })).not.toBeInTheDocument()
    expect(detectCalls).toContainEqual({ workspace: "/ws", language: "python" })
  })

  it("probes installed state again when Settings is reopened", async () => {
    const first = renderDialog({ initialSection: "lsp" })
    await waitFor(() =>
      expect(detectCalls.filter((call) => call.language === "python")).toHaveLength(1),
    )

    first.unmount()
    renderDialog({ initialSection: "lsp" })

    await waitFor(() =>
      expect(detectCalls.filter((call) => call.language === "python")).toHaveLength(2),
    )
  })

  it("manual re-detect probes the current scope again", async () => {
    renderDialog({ initialSection: "lsp" })
    const card = await screen.findByTestId("lsp-card-python")
    await waitFor(() =>
      expect(detectCalls.filter((call) => call.language === "python")).toHaveLength(1),
    )

    fireEvent.click(within(card).getByRole("button", { name: "重新偵測" }))

    await waitFor(() =>
      expect(detectCalls.filter((call) => call.language === "python")).toHaveLength(2),
    )
    expect(detectCalls.filter((call) => call.language === "python").at(-1)).toEqual({
      workspace: "/ws",
      language: "python",
    })
  })

  it("global detection uses null and ignores a late workspace result", async () => {
    let resolveWorkspacePython!: (info: LspServerInfo) => void
    detectHandler = (workspace, language) => {
      if (workspace === "/ws" && language === "python") {
        return new Promise<LspServerInfo>((resolve) => {
          resolveWorkspacePython = resolve
        })
      }
      return serverInfo(language, {
        workspace: workspace ?? "",
        serverId: workspace == null ? `global-${language}` : `workspace-${language}`,
        path: workspace == null && language === "python" ? "/global/pyright" : null,
        status:
          workspace == null && language === "python"
            ? { status: "stopped" }
            : { status: "missing", installHint: "install" },
      })
    }

    renderDialog({ initialSection: "lsp" })
    const card = await screen.findByTestId("lsp-card-python")
    await waitFor(() =>
      expect(detectCalls).toContainEqual({ workspace: "/ws", language: "python" }),
    )

    fireEvent.click(screen.getByRole("radio", { name: "全域" }))
    await waitFor(() =>
      expect(detectCalls).toContainEqual({ workspace: null, language: "python" }),
    )
    expect(await within(card).findByText("已安裝")).toBeInTheDocument()
    expect(within(card).getByText("global-python")).toBeInTheDocument()

    await act(async () => {
      resolveWorkspacePython(
        serverInfo("python", {
          serverId: "late-workspace-python",
          path: "/workspace/pyright",
          status: { status: "stopped" },
        }),
      )
    })

    expect(within(card).getByText("global-python")).toBeInTheDocument()
    expect(within(card).queryByText("late-workspace-python")).not.toBeInTheDocument()
  })

  it("keeps the global view isolated from workspace lifecycle store updates", async () => {
    renderDialog({ initialSection: "lsp" })
    const card = await screen.findByTestId("lsp-card-python")

    fireEvent.click(screen.getByRole("radio", { name: "全域" }))
    await waitFor(() =>
      expect(detectCalls).toContainEqual({ workspace: null, language: "python" }),
    )
    expect(await within(card).findByText("未安裝")).toBeInTheDocument()

    act(() => {
      useLspStore.getState().setServerInfo(
        serverInfo("python", {
          workspace: "/ws",
          path: "/workspace/pyright",
          status: { status: "stopped" },
        }),
      )
    })

    expect(within(card).getByText("未安裝")).toBeInTheDocument()
    expect(within(card).queryByText("已安裝")).not.toBeInTheDocument()
  })

  it("does not retain the previous scope when the current probe fails", async () => {
    detectHandler = (workspace, language) => {
      if (workspace == null) return Promise.reject(new Error("probe failed"))
      return serverInfo(language, {
        workspace,
        path: language === "python" ? "/workspace/pyright" : null,
        status:
          language === "python"
            ? { status: "stopped" }
            : { status: "missing", installHint: "install" },
      })
    }

    renderDialog({ initialSection: "lsp" })
    const card = await screen.findByTestId("lsp-card-python")
    expect(await within(card).findByText("已安裝")).toBeInTheDocument()

    fireEvent.click(screen.getByRole("radio", { name: "全域" }))
    await waitFor(() =>
      expect(detectCalls).toContainEqual({ workspace: null, language: "python" }),
    )

    expect(await within(card).findByText("尚未啟動")).toBeInTheDocument()
    expect(within(card).queryByText("已安裝")).not.toBeInTheDocument()
    expect(within(card).queryByText("/workspace/pyright")).not.toBeInTheDocument()
  })

  it("switches a profile with workspace scope → lspConfigSetServer(workspace, …)", async () => {
    renderDialog({ initialSection: "lsp" })
    const card = await screen.findByTestId("lsp-card-python")

    // Scope defaults to workspace when a workspace is open.
    fireEvent.click(within(card).getByRole("radio", { name: "pylsp" }))

    await waitFor(() =>
      expect(setServerCalls).toContainEqual({
        workspace: "/ws",
        language: "python",
        serverId: "pylsp",
      }),
    )
  })

  it("switches a profile with global scope → lspConfigSetServer(null, …)", async () => {
    renderDialog({ initialSection: "lsp" })
    await screen.findByTestId("lsp-card-python")

    fireEvent.click(screen.getByRole("radio", { name: "全域" }))
    fireEvent.click(within(screen.getByTestId("lsp-card-python")).getByRole("radio", { name: "pylsp" }))

    await waitFor(() =>
      expect(setServerCalls).toContainEqual({
        workspace: null,
        language: "python",
        serverId: "pylsp",
      }),
    )
  })

  it("shows rust's single profile as a fixed (disabled) option", async () => {
    renderDialog({ initialSection: "lsp" })
    const card = await screen.findByTestId("lsp-card-rust")

    expect(within(card).getByRole("radio", { name: "rust-analyzer" })).toBeDisabled()
  })

  it("format-on-save defaults OFF and persists to localStorage", async () => {
    renderDialog({ initialSection: "lsp" })

    const sw = await screen.findByRole("switch", { name: "儲存時自動格式化" })
    expect(sw).not.toBeChecked()

    fireEvent.click(sw)
    expect(localStorage.getItem(FORMAT_ON_SAVE_STORAGE_KEY)).toBe("true")
  })

  it("JSON-RPC trace toggle defaults off and calls lspSetTrace", async () => {
    renderDialog({ initialSection: "lsp" })

    const sw = await screen.findByRole("switch", { name: "JSON-RPC 追蹤" })
    expect(sw).not.toBeChecked()

    fireEvent.click(sw)
    await waitFor(() => expect(traceCalls).toEqual([true]))
  })

  it("one-click install forwards the current workspace to lspInstallServer (W6A-F1)", async () => {
    renderDialog({ initialSection: "lsp" })

    const card = await screen.findByTestId("lsp-card-python")
    fireEvent.click(within(card).getByRole("button", { name: "一鍵安裝" }))

    // Must carry the workspace so the Rust side resolves the workspace override
    // (else a pylsp override installs the global-default pyright — dead end).
    await waitFor(() => expect(installCalls).toEqual([{ workspace: "/ws", language: "python" }]))
  })

  it("one-click install forwards null workspace when none is open (W6A-F1)", async () => {
    useWorkspaceStore.setState({ workspacePath: null })
    renderDialog({ initialSection: "lsp" })

    const card = await screen.findByTestId("lsp-card-python")
    fireEvent.click(within(card).getByRole("button", { name: "一鍵安裝" }))

    await waitFor(() => expect(installCalls).toEqual([{ workspace: null, language: "python" }]))
  })

  it("one-click install follows global scope and ignores its result after switching scope", async () => {
    useInstallDeferred = true
    renderDialog({ initialSection: "lsp" })
    const card = await screen.findByTestId("lsp-card-python")

    fireEvent.click(screen.getByRole("radio", { name: "全域" }))
    fireEvent.click(within(card).getByRole("button", { name: "一鍵安裝" }))
    await waitFor(() => expect(installCalls).toEqual([{ workspace: null, language: "python" }]))

    fireEvent.click(screen.getByRole("radio", { name: "此工作區" }))
    await act(async () => {
      installResolvers!.resolve(
        serverInfo("python", {
          workspace: "",
          serverId: "global-install",
          status: { status: "stopped" },
        }),
      )
    })

    expect(useLspStore.getState().servers.python?.serverId).not.toBe("global-install")
  })

  it("surfaces actionable install diagnostics and allows retry", async () => {
    installReject =
      "npm install 失敗（工具 npm；exit 7）\nstderr（已去敏，末尾）：registry unavailable"
    renderDialog({ initialSection: "lsp" })

    const card = await screen.findByTestId("lsp-card-python")
    fireEvent.click(within(card).getByRole("button", { name: "一鍵安裝" }))

    const failedCard = screen.getByTestId("lsp-card-python")
    const diagnostic = await within(failedCard).findByText(/npm install 失敗/)
    expect(diagnostic).toHaveTextContent("工具 npm；exit 7")
    expect(diagnostic).toHaveTextContent("stderr（已去敏，末尾）：registry unavailable")
    expect(within(failedCard).getByRole("button", { name: "一鍵安裝" })).toBeEnabled()
  })

  it("renders install progress from lsp:install-progress events", async () => {
    renderDialog({ initialSection: "lsp" })
    await screen.findByTestId("lsp-card-python")
    await waitFor(() => expect(listeners.has("lsp:install-progress")).toBe(true))

    act(() => {
      listeners.get("lsp:install-progress")!({
        payload: { language: "python", phase: "npm", percent: 42, message: "installing" },
      })
    })

    expect(within(screen.getByTestId("lsp-card-python")).getByText(/42/)).toBeInTheDocument()
  })

  it("keeps the install button usable after a terminal 'done' event races the promise (A-F1)", async () => {
    useInstallDeferred = true
    renderDialog({ initialSection: "lsp" })
    const card = await screen.findByTestId("lsp-card-python")
    await waitFor(() => expect(listeners.has("lsp:install-progress")).toBe(true))

    fireEvent.click(within(card).getByRole("button", { name: "一鍵安裝" }))
    await waitFor(() =>
      expect(
        within(screen.getByTestId("lsp-card-python")).getByRole("button", { name: "安裝中…" }),
      ).toBeDisabled(),
    )

    // The install call settles (handleInstall's finally clears installing)…
    await act(async () => {
      installResolvers!.resolve(serverInfo("python"))
    })
    // …then a late terminal event arrives — it must NOT re-disable the button.
    act(() => {
      listeners.get("lsp:install-progress")!({
        payload: { language: "python", phase: "done", percent: 100, message: null },
      })
    })

    expect(
      within(screen.getByTestId("lsp-card-python")).getByRole("button", { name: "一鍵安裝" }),
    ).toBeEnabled()
  })

  it("skips the store write when the workspace changed during install (M3F-3)", async () => {
    useInstallDeferred = true
    useWorkspaceStore.setState({ workspacePath: "/ws" })
    renderDialog({ initialSection: "lsp" })
    const card = await screen.findByTestId("lsp-card-python")

    fireEvent.click(within(card).getByRole("button", { name: "一鍵安裝" }))
    await waitFor(() => expect(installCalls).toHaveLength(1))

    // User leaves for a different workspace before the install settles.
    act(() => useWorkspaceStore.setState({ workspacePath: "/other" }))
    await act(async () => {
      installResolvers!.resolve(serverInfo("python", { serverId: "pyright" }))
    })

    // The new workspace's detection may populate the store, but the stale
    // install result from /ws must never replace it.
    expect(useLspStore.getState().servers.python?.serverId).not.toBe("pyright")
    expect(useLspStore.getState().servers.python?.workspace).toBe("/other")
  })

  it("writes installed state immediately when the current view is unchanged", async () => {
    useInstallDeferred = true
    useWorkspaceStore.setState({ workspacePath: "/ws" })
    renderDialog({ initialSection: "lsp" })
    const card = await screen.findByTestId("lsp-card-python")

    fireEvent.click(within(card).getByRole("button", { name: "一鍵安裝" }))
    await waitFor(() => expect(installCalls).toHaveLength(1))

    await act(async () => {
      installResolvers!.resolve(
        serverInfo("python", { serverId: "pyright", status: { status: "stopped" } }),
      )
    })

    expect(useLspStore.getState().servers.python?.serverId).toBe("pyright")
    expect(within(card).getByText("已安裝")).toBeInTheDocument()
    expect(within(card).queryByRole("button", { name: "一鍵安裝" })).not.toBeInTheDocument()
  })

  it("does not let an older detection overwrite a successful install", async () => {
    let resolveDetection!: (info: LspServerInfo) => void
    detectHandler = (workspace, language) => {
      if (language === "python") {
        return new Promise<LspServerInfo>((resolve) => {
          resolveDetection = resolve
        })
      }
      return serverInfo(language, {
        workspace: workspace ?? "",
        path: null,
        status: { status: "missing", installHint: "install" },
      })
    }
    useInstallDeferred = true
    renderDialog({ initialSection: "lsp" })
    const card = await screen.findByTestId("lsp-card-python")
    await waitFor(() =>
      expect(detectCalls).toContainEqual({ workspace: "/ws", language: "python" }),
    )

    fireEvent.click(within(card).getByRole("button", { name: "一鍵安裝" }))
    await act(async () => {
      installResolvers!.resolve(
        serverInfo("python", { serverId: "pyright", status: { status: "stopped" } }),
      )
    })
    await act(async () => {
      resolveDetection(
        serverInfo("python", {
          path: null,
          status: { status: "missing", installHint: "install" },
        }),
      )
    })

    expect(useLspStore.getState().servers.python?.status.status).toBe("stopped")
    expect(within(card).getByText("已安裝")).toBeInTheDocument()
  })

  it("reverts the JSON-RPC trace toggle when lspSetTrace rejects (A-F3)", async () => {
    traceReject = true
    renderDialog({ initialSection: "lsp" })
    const sw = await screen.findByRole("switch", { name: "JSON-RPC 追蹤" })
    expect(sw).not.toBeChecked()

    fireEvent.click(sw)
    await waitFor(() => expect(traceCalls).toEqual([true]))
    await waitFor(() => expect(sw).not.toBeChecked()) // optimistic on → revert off
  })

  it("a superseded trace reject must not clobber a newer toggle (R2A-F1 gen guard)", async () => {
    // Three-click repro from the review counterexample: a two-toggle alternating
    // case is non-discriminating (prev1 === click2's target), so use three.
    traceDeferred = true
    renderDialog({ initialSection: "lsp" })
    const sw = await screen.findByRole("switch", { name: "JSON-RPC 追蹤" })

    fireEvent.click(sw) // click1 → true  (P1, will reject late)
    fireEvent.click(sw) // click2 → false (P2, resolves)
    fireEvent.click(sw) // click3 → true  (P3, resolves)
    await waitFor(() => expect(traceCalls).toEqual([true, false, true]))
    expect(traceResolvers).toHaveLength(3)

    // Newer requests settle first, then the stale P1 rejects late.
    await act(async () => {
      traceResolvers[1].resolve()
      traceResolvers[2].resolve()
      traceResolvers[0].reject("late P1 failure")
    })

    // Final state must be click3's value (true) — the stale P1 revert to its own
    // prev (false) must be ignored because it is no longer the latest request.
    expect(useUiStore.getState().traceEnabled).toBe(true)
    expect(sw).toBeChecked()
  })

  it("keeps trace state across dialog close/reopen (store-backed, A-F4)", async () => {
    renderDialog({ initialSection: "lsp" })
    fireEvent.click(await screen.findByRole("switch", { name: "JSON-RPC 追蹤" }))
    await waitFor(() => expect(traceCalls).toEqual([true]))

    cleanup() // unmount the pane (dialog close)
    renderDialog({ initialSection: "lsp" })
    expect(await screen.findByRole("switch", { name: "JSON-RPC 追蹤" })).toBeChecked()
  })

  it("unsubscribes the install-progress listener on unmount (A-F6)", async () => {
    const { unmount } = renderDialog({ initialSection: "lsp" })
    await waitFor(() => expect(unlistenSpies.has("lsp:install-progress")).toBe(true))
    const spy = unlistenSpies.get("lsp:install-progress")!

    unmount()
    await waitFor(() => expect(spy).toHaveBeenCalled())
  })

  it("highlights the targeted language card when opened with a language (A-F6)", async () => {
    renderDialog({ initialSection: "lsp", initialLanguage: "python" })
    const card = await screen.findByTestId("lsp-card-python")

    expect(card).toHaveAttribute("data-highlighted", "true")
    expect(screen.getByTestId("lsp-card-rust")).not.toHaveAttribute("data-highlighted")
  })

  it("lists stale workspace overrides and clears one", async () => {
    const rawWorkspace = String.raw`\\?\UNC\Server\Share\中文 workspace`
    const displayWorkspace = String.raw`\\Server\Share\中文 workspace`
    staleResult = [rawWorkspace]
    renderDialog({ initialSection: "lsp" })

    const stale = await screen.findByTestId("lsp-stale")
    expect(within(stale).getByText(displayWorkspace)).toBeInTheDocument()
    expect(within(stale).queryByText(rawWorkspace)).not.toBeInTheDocument()

    fireEvent.click(within(stale).getByRole("button", { name: "清除" }))
    await waitFor(() => expect(clearStaleCalls).toEqual([rawWorkspace]))
  })

  it("re-applies the section when the target prop changes while open", async () => {
    const { rerender } = renderDialog({ initialSection: "appearance" })
    expect(screen.getByRole("heading", { name: "Appearance" })).toBeInTheDocument()

    rerender(
      <SettingsDialog
        open
        onOpenChange={() => {}}
        theme="light"
        onThemeChange={() => {}}
        initialSection="lsp"
        initialLanguage="python"
      />,
    )

    expect(await screen.findByRole("heading", { name: "LSP" })).toBeInTheDocument()
    expect(screen.getByTestId("lsp-card-python")).toBeInTheDocument()
  })
})

describe("SettingsDialog terminal and preview sections", () => {
  it("renders terminal and preview nav entries", () => {
    renderDialog()

    expect(screen.getByRole("button", { name: "Terminal" })).toBeInTheDocument()
    expect(screen.getByRole("button", { name: "Preview" })).toBeInTheDocument()
  })

  it("persists a custom terminal executable and structured args", () => {
    renderDialog({ initialSection: "terminal" })

    fireEvent.change(screen.getByRole("combobox", { name: "Default profile" }), {
      target: { value: "custom" },
    })
    fireEvent.change(screen.getByLabelText("Custom executable"), {
      target: { value: "/opt/homebrew/bin/fish" },
    })
    fireEvent.change(screen.getByLabelText("Custom arguments"), {
      target: { value: "-l\n--private" },
    })

    expect(loadTerminalSettings()).toMatchObject({
      defaultProfile: {
        id: "custom",
        shell: "/opt/homebrew/bin/fish",
        args: ["-l", "--private"],
      },
    })
  })

  it("persists Terminal size scope separately and seeds the active workspace ratio", () => {
    useWorkbenchLayoutStore.getState().setTerminalRatio(null, 0.64)
    renderDialog({ initialSection: "terminal" })

    expect(screen.getByRole("radio", { name: "Global" })).toBeChecked()
    fireEvent.click(screen.getByRole("radio", { name: "Per workspace" }))

    expect(screen.getByRole("radio", { name: "Per workspace" })).toBeChecked()
    expect(useWorkbenchLayoutStore.getState().terminalWorkspaceRatios).toEqual({ "/ws": 0.64 })
    expect(localStorage.getItem(TERMINAL_SETTINGS_STORAGE_KEY)).toBeNull()
    expect(JSON.parse(localStorage.getItem(WORKBENCH_LAYOUT_STORAGE_KEY)!)).toEqual({
      version: 1,
      markdownEditorRatio: 0.5,
      terminalRatioScope: "workspace",
      terminalGlobalRatio: 0.64,
      terminalWorkspaceRatios: { "/ws": 0.64 },
    })
  })

  it("changes only Terminal size scope when no workspace is open", () => {
    useWorkspaceStore.setState({ workspacePath: null })
    useWorkbenchLayoutStore.getState().setTerminalRatio(null, 0.48)
    renderDialog({ initialSection: "terminal" })

    fireEvent.click(screen.getByRole("radio", { name: "Per workspace" }))

    const layout = useWorkbenchLayoutStore.getState()
    expect(layout.terminalRatioScope).toBe("workspace")
    expect(layout.terminalWorkspaceRatios).toEqual({})
    expect(layout.effectiveTerminalRatio("/later")).toBe(0.48)
  })

  it("persists preview command and port overrides", () => {
    renderDialog({ initialSection: "preview" })

    fireEvent.change(screen.getByLabelText("Dev server command override"), {
      target: { value: "bun run dev -- --host 127.0.0.1" },
    })
    fireEvent.change(screen.getByLabelText("Port override"), {
      target: { value: "6000" },
    })

    expect(localStorage.getItem(PREVIEW_SETTINGS_STORAGE_KEY)).toBe(
      JSON.stringify({ command: "bun run dev -- --host 127.0.0.1", port: "6000" })
    )
  })

  it("openSettings preview target shows the Preview section", () => {
    renderDialog({ initialSection: "preview", openNonce: 1 })

    expect(screen.getByRole("heading", { name: "Preview" })).toBeInTheDocument()
    expect(screen.getByLabelText("Dev server command override")).toBeInTheDocument()
  })
})

describe("openSettings API contract (T11 consumer)", () => {
  it("openSettings('lsp','python') opens on the LSP section with the python card", async () => {
    // No workspace open (matches appShell.test default): AppShell's FileTree
    // renders its empty state instead of trying to map an unloaded tree.
    useWorkspaceStore.setState({ workspacePath: null })
    render(<AppShell />)

    act(() => {
      useUiStore.getState().openSettings("lsp", "python")
    })

    expect(await screen.findByRole("dialog")).toBeInTheDocument()
    expect(screen.getByRole("heading", { name: "LSP" })).toBeInTheDocument()
    expect(screen.getByTestId("lsp-card-python")).toBeInTheDocument()
  })

  it("re-opens on the LSP section after the user navigated away (A-F2 nonce)", async () => {
    useWorkspaceStore.setState({ workspacePath: null })
    render(<AppShell />)
    act(() => useUiStore.getState().openSettings("lsp", "python"))
    const dialog = await screen.findByRole("dialog")
    expect(within(dialog).getByRole("heading", { name: "LSP" })).toBeInTheDocument()

    // Manual nav to another section inside the still-open dialog.
    fireEvent.click(within(dialog).getByRole("button", { name: "Git" }))
    expect(within(dialog).getByRole("heading", { name: "Git" })).toBeInTheDocument()

    // Same-target openSettings must re-fire the sync effect and restore LSP.
    act(() => useUiStore.getState().openSettings("lsp", "python"))
    expect(await within(dialog).findByRole("heading", { name: "LSP" })).toBeInTheDocument()
  })

  it("clears the language highlight after manual re-entry to the LSP section (A-F5)", async () => {
    useWorkspaceStore.setState({ workspacePath: null })
    render(<AppShell />)
    act(() => useUiStore.getState().openSettings("lsp", "python"))
    const dialog = await screen.findByRole("dialog")
    expect(within(dialog).getByTestId("lsp-card-python")).toHaveAttribute("data-highlighted", "true")

    fireEvent.click(within(dialog).getByRole("button", { name: "Appearance" }))
    fireEvent.click(within(dialog).getByRole("button", { name: "LSP" }))

    expect(within(dialog).getByTestId("lsp-card-python")).not.toHaveAttribute("data-highlighted")
  })
})

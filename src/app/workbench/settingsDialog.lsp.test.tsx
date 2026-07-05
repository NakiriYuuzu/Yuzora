import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"
import { act, cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react"
import { clearMocks, mockIPC } from "@tauri-apps/api/mocks"

import { AppShell } from "@/app/AppShell"
import { SettingsDialog } from "@/app/workbench/SettingsDialog"
import { FORMAT_ON_SAVE_STORAGE_KEY } from "@/editor/EditorPane"
import type { LspConfig, LspServerInfo } from "@/lib/types"
import { useLspStore } from "@/state/lspStore"
import { uiInitialState, useUiStore } from "@/state/uiStore"
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

  it("surfaces the install failure message (no ecosystem-detection API)", async () => {
    installReject = "npm not found"
    renderDialog({ initialSection: "lsp" })

    const card = await screen.findByTestId("lsp-card-python")
    fireEvent.click(within(card).getByRole("button", { name: "一鍵安裝" }))

    expect(
      await within(screen.getByTestId("lsp-card-python")).findByText(/npm not found/),
    ).toBeInTheDocument()
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
    useWorkspaceStore.setState({ workspacePath: "/other" })
    await act(async () => {
      installResolvers!.resolve(serverInfo("python", { serverId: "pyright" }))
    })

    // Stale result must not pollute the (now different) workspace's store.
    expect(useLspStore.getState().servers.python).toBeUndefined()
  })

  it("writes to the store when the workspace is unchanged at settle (M3F-3 positive)", async () => {
    useInstallDeferred = true
    useWorkspaceStore.setState({ workspacePath: "/ws" })
    renderDialog({ initialSection: "lsp" })
    const card = await screen.findByTestId("lsp-card-python")

    fireEvent.click(within(card).getByRole("button", { name: "一鍵安裝" }))
    await waitFor(() => expect(installCalls).toHaveLength(1))

    await act(async () => {
      installResolvers!.resolve(serverInfo("python", { serverId: "pyright" }))
    })

    expect(useLspStore.getState().servers.python?.serverId).toBe("pyright")
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
    staleResult = ["/gone/ws"]
    renderDialog({ initialSection: "lsp" })

    const stale = await screen.findByTestId("lsp-stale")
    expect(within(stale).getByText("/gone/ws")).toBeInTheDocument()

    fireEvent.click(within(stale).getByRole("button", { name: "清除" }))
    await waitFor(() => expect(clearStaleCalls).toEqual(["/gone/ws"]))
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

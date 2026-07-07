import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"
import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react"

import { PreviewPanel } from "@/app/panels/PreviewPanel"
import i18n from "@/lib/i18n"
import { usePreviewStore } from "@/state/previewStore"
import { useWorkspaceStore } from "@/state/workspaceStore"

// This file focuses on coverage panels.test.tsx's "PreviewPanel dev server flow"
// describe block doesn't already have: the no-candidates branch, an IPC-rejection
// error path, and the native child-webview lifecycle (open/bounds/visible/close)
// gated by isTauri(). Basic detect->start and stop flows are included too, kept
// deliberately small since the happy paths are already covered there.

const ipcMocks = vi.hoisted(() => ({
  devServerDetect: vi.fn(),
  devServerStart: vi.fn(),
  devServerStop: vi.fn(),
  previewOpenUrl: vi.fn(),
  previewSetBounds: vi.fn(),
  previewSetVisible: vi.fn(),
  previewClose: vi.fn(),
}))

vi.mock("@/lib/ipc", async (importOriginal) => ({
  ...(await importOriginal<typeof import("@/lib/ipc")>()),
  devServerDetect: (...args: unknown[]) => ipcMocks.devServerDetect(...args),
  devServerStart: (...args: unknown[]) => ipcMocks.devServerStart(...args),
  devServerStop: (...args: unknown[]) => ipcMocks.devServerStop(...args),
  previewOpenUrl: (...args: unknown[]) => ipcMocks.previewOpenUrl(...args),
  previewSetBounds: (...args: unknown[]) => ipcMocks.previewSetBounds(...args),
  previewSetVisible: (...args: unknown[]) => ipcMocks.previewSetVisible(...args),
  previewClose: (...args: unknown[]) => ipcMocks.previewClose(...args),
}))

// The Bun-hosted test runtime injects an empty `localStorage` global with no
// Storage methods (see sshPanel.test.tsx); install a minimal in-memory Storage
// so loadPreviewSettings' read/write round-trips run for real.
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

const startButton = () => screen.getByRole("button", { name: i18n.t("start", { ns: "preview" }) })

beforeEach(() => {
  installLocalStorage()
  useWorkspaceStore.setState({ workspacePath: "/workspace" })
})

afterEach(() => {
  cleanup()
  usePreviewStore.getState().reset()
  useWorkspaceStore.setState({ workspacePath: null })
  delete (globalThis as { isTauri?: boolean }).isTauri
  vi.clearAllMocks()
})

describe("PreviewPanel dev-server detection", () => {
  it("renders the no-candidates guidance when detection finds nothing to run", async () => {
    ipcMocks.devServerDetect.mockResolvedValueOnce({ candidates: [], runningPorts: [] })

    render(<PreviewPanel />)
    fireEvent.click(startButton())

    expect(await screen.findByText(i18n.t("noCandidates", { ns: "preview" }))).toBeInTheDocument()
    expect(
      screen.getByRole("button", { name: i18n.t("retryDetect", { ns: "preview" }) })
    ).toBeInTheDocument()
    expect(ipcMocks.devServerStart).not.toHaveBeenCalled()
  })

  it("detects a candidate and auto-starts it, landing on the running status", async () => {
    ipcMocks.devServerDetect.mockResolvedValueOnce({
      candidates: [{ scriptName: "dev", command: "bun run dev:web", likelyPort: 4173 }],
      runningPorts: [],
    })
    ipcMocks.devServerStart.mockResolvedValueOnce({
      workspace: "/workspace",
      command: "bun run dev:web",
      port: 4173,
      status: { status: "running", port: 4173 },
    })

    render(<PreviewPanel />)
    fireEvent.click(startButton())

    await waitFor(() =>
      expect(ipcMocks.devServerStart).toHaveBeenCalledWith(
        "/workspace",
        "bun run dev:web",
        4173,
        expect.any(Function)
      )
    )
    expect(
      await screen.findByText(i18n.t("previewPanel.status.running", { ns: "panels" }))
    ).toBeInTheDocument()
  })
})

describe("PreviewPanel start/stop lifecycle", () => {
  it("stops a running dev server via the Stop button", async () => {
    usePreviewStore.getState().setDevServer({
      workspace: "/workspace",
      command: "bun run dev:web",
      port: 4173,
      status: { status: "running", port: 4173 },
    })
    ipcMocks.devServerStop.mockResolvedValueOnce(undefined)

    render(<PreviewPanel />)
    fireEvent.click(
      screen.getByRole("button", { name: i18n.t("previewPanel.stop", { ns: "panels" }) })
    )

    await waitFor(() => expect(ipcMocks.devServerStop).toHaveBeenCalledWith("/workspace"))
    expect(
      await screen.findByText(i18n.t("previewPanel.status.exited", { ns: "panels" }))
    ).toBeInTheDocument()
  })
})

describe("PreviewPanel error path", () => {
  it("shows a failed status with the rejection reason when devServerStart rejects, without crashing", async () => {
    ipcMocks.devServerDetect.mockResolvedValueOnce({
      candidates: [{ scriptName: "dev", command: "bun run dev:web", likelyPort: 4173 }],
      runningPorts: [],
    })
    ipcMocks.devServerStart.mockRejectedValueOnce(new Error("spawn ENOENT"))

    render(<PreviewPanel />)
    fireEvent.click(startButton())

    expect(await screen.findByText("spawn ENOENT")).toBeInTheDocument()
    expect(
      screen.getByRole("button", { name: i18n.t("retryStart", { ns: "preview" }) })
    ).toBeInTheDocument()
  })
})

describe("PreviewPanel native child-webview lifecycle (Tauri only)", () => {
  it("opens the webview, syncs bounds/visibility on mount, and closes it on unmount", async () => {
    ;(globalThis as { isTauri?: boolean }).isTauri = true
    usePreviewStore.getState().navigate("/workspace", "https://example.com")

    const { unmount } = render(<PreviewPanel />)

    await waitFor(() =>
      expect(ipcMocks.previewOpenUrl).toHaveBeenCalledWith("https://example.com", 0, 0, 0, 0)
    )
    expect(ipcMocks.previewSetBounds).toHaveBeenCalledWith(0, 0, 0, 0)
    expect(ipcMocks.previewSetVisible).toHaveBeenCalledWith(true)
    expect(ipcMocks.previewClose).not.toHaveBeenCalled()

    unmount()
    expect(ipcMocks.previewClose).toHaveBeenCalled()
  })

  it("does not touch the native webview when running outside Tauri", () => {
    usePreviewStore.getState().navigate("/workspace", "https://example.com")

    render(<PreviewPanel />)

    expect(ipcMocks.previewOpenUrl).not.toHaveBeenCalled()
    expect(ipcMocks.previewSetBounds).not.toHaveBeenCalled()
    expect(ipcMocks.previewSetVisible).not.toHaveBeenCalled()
    expect(ipcMocks.previewClose).not.toHaveBeenCalled()
  })
})

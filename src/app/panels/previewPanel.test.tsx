import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"
import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react"

import { PreviewPanel } from "@/app/panels/PreviewPanel"
import i18n from "@/lib/i18n"
import { useContextMenuStore } from "@/state/contextMenuStore"
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
  previewBack: vi.fn(),
  previewForward: vi.fn(),
  previewReload: vi.fn(),
  openUrl: vi.fn(),
  showActionError: vi.fn(),
}))

vi.mock("@tauri-apps/plugin-opener", () => ({
  openUrl: (...args: unknown[]) => ipcMocks.openUrl(...args),
}))

vi.mock("@/lib/actionFeedback", () => ({
  showActionError: (...args: unknown[]) => ipcMocks.showActionError(...args),
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
  previewBack: (...args: unknown[]) => ipcMocks.previewBack(...args),
  previewForward: (...args: unknown[]) => ipcMocks.previewForward(...args),
  previewReload: (...args: unknown[]) => ipcMocks.previewReload(...args),
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

function deferred<T>() {
  let resolve!: (value: T | PromiseLike<T>) => void
  let reject!: (reason?: unknown) => void
  const promise = new Promise<T>((resolvePromise, rejectPromise) => {
    resolve = resolvePromise
    reject = rejectPromise
  })
  return { promise, resolve, reject }
}

beforeEach(() => {
  installLocalStorage()
  useWorkspaceStore.setState({ workspacePath: "/workspace" })
  useContextMenuStore.setState({ request: null, x: 0, y: 0, availabilityRevision: 0 })
})

afterEach(async () => {
  cleanup()
  await new Promise((resolve) => setTimeout(resolve, 0))
  usePreviewStore.getState().reset()
  useWorkspaceStore.setState({ workspacePath: null })
  useContextMenuStore.setState({ request: null })
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
    await waitFor(() => expect(ipcMocks.previewClose).toHaveBeenCalled())
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

describe("PreviewPanel context-menu boundary", () => {
  it("opens a typed preview request from toolbar and empty/error chrome", () => {
    const { unmount } = render(<PreviewPanel />)

    fireEvent.contextMenu(screen.getByTestId("preview-toolbar"), { clientX: 12, clientY: 24 })
    expect(useContextMenuStore.getState()).toMatchObject({
      request: {
        kind: "preview",
        workspacePath: "/workspace",
        url: null,
        serverAttempt: 0,
      },
      x: 12,
      y: 24,
    })

    useContextMenuStore.getState().close()
    fireEvent.contextMenu(screen.getByTestId("preview-empty-chrome"))
    expect(useContextMenuStore.getState().request?.kind).toBe("preview")
    unmount()

    useContextMenuStore.getState().close()
    usePreviewStore.getState().setDevServer({
      workspace: "/workspace",
      command: "bun run dev",
      port: 5173,
      status: { status: "failed", reason: "spawn failed" },
    })
    render(<PreviewPanel />)
    fireEvent.contextMenu(screen.getByTestId("preview-error-chrome"))
    expect(useContextMenuStore.getState().request?.kind).toBe("preview")
  })

  it("does not attach the Yuzora preview menu to iframe/frame shell or native host", () => {
    usePreviewStore.getState().navigate("/workspace", "http://localhost:5173")
    const { unmount } = render(<PreviewPanel />)

    fireEvent.contextMenu(screen.getByTestId("preview-frame-shell"))
    expect(useContextMenuStore.getState().request).toBeNull()
    fireEvent.contextMenu(screen.getByTitle("Live preview"))
    expect(useContextMenuStore.getState().request).toBeNull()
    unmount()

    usePreviewStore.getState().reset()
    usePreviewStore.getState().navigate("/workspace", "https://example.com")
    render(<PreviewPanel />)
    fireEvent.contextMenu(screen.getByTestId("preview-webview-host"))
    expect(useContextMenuStore.getState().request).toBeNull()
  })
})

describe("PreviewPanel shared toolbar commands", () => {
  it("routes toolbar navigation through the shared local/native commands", async () => {
    const preview = usePreviewStore.getState()
    preview.navigate("/workspace", "https://example.com/first")
    preview.navigate("/workspace", "https://example.com/second")
    render(<PreviewPanel />)

    fireEvent.click(screen.getByRole("button", { name: i18n.t("previewPanel.back", { ns: "panels" }) }))
    await waitFor(() => {
      expect(usePreviewStore.getState().navForWorkspace("/workspace").url)
        .toBe("https://example.com/first")
    })
    expect(ipcMocks.previewBack).not.toHaveBeenCalled()
    preview.recordNativeOpen("/workspace", "https://example.com/first")
    fireEvent.click(screen.getByRole("button", { name: i18n.t("previewPanel.reload", { ns: "panels" }) }))
    await waitFor(() => expect(ipcMocks.previewReload).toHaveBeenCalledTimes(1))
    expect(ipcMocks.previewReload).toHaveBeenCalledTimes(1)

    cleanup()
    usePreviewStore.getState().reset()
    preview.navigate("/workspace", "http://localhost:5173")
    preview.navigate("/workspace", "http://localhost:5173/about")
    render(<PreviewPanel />)
    fireEvent.click(screen.getByRole("button", { name: i18n.t("previewPanel.back", { ns: "panels" }) }))
    await waitFor(() => {
      expect(usePreviewStore.getState().navForWorkspace("/workspace").url)
        .toBe("http://localhost:5173")
    })
  })

  it("switches external history back to the local iframe without a native no-op", async () => {
    const preview = usePreviewStore.getState()
    preview.navigate("/workspace", "http://localhost:8765/")
    preview.navigate("/workspace", "https://example.com/")
    render(<PreviewPanel />)

    fireEvent.click(screen.getByRole("button", { name: i18n.t("previewPanel.back", { ns: "panels" }) }))

    await waitFor(() => {
      expect(screen.getByLabelText(i18n.t("previewPanel.urlLabel", { ns: "panels" })))
        .toHaveValue("http://localhost:8765/")
    })
    expect(screen.getByTitle("Live preview")).toHaveAttribute("src", "http://localhost:8765/")
    expect(ipcMocks.previewBack).not.toHaveBeenCalled()
  })

  it("syncs external native Back/Forward without reopening the URL through previewOpenUrl", async () => {
    ;(globalThis as { isTauri?: boolean }).isTauri = true
    const preview = usePreviewStore.getState()
    preview.navigate("/workspace", "https://example.com/a")
    render(<PreviewPanel />)
    await waitFor(() => expect(ipcMocks.previewOpenUrl).toHaveBeenCalledTimes(1))
    await waitFor(() => expect(usePreviewStore.getState().nativeSession?.currentUrl)
      .toBe("https://example.com/a"))
    preview.navigate("/workspace", "https://example.com/b")
    await waitFor(() => expect(ipcMocks.previewOpenUrl).toHaveBeenCalledTimes(2))
    await waitFor(() => expect(usePreviewStore.getState().nativeSession).toMatchObject({
      currentUrl: "https://example.com/b",
      backStack: ["https://example.com/a"],
    }))
    ipcMocks.previewOpenUrl.mockClear()

    fireEvent.click(screen.getByRole("button", { name: i18n.t("previewPanel.back", { ns: "panels" }) }))
    await waitFor(() => {
      expect(usePreviewStore.getState().navForWorkspace("/workspace").url)
        .toBe("https://example.com/a")
      expect(usePreviewStore.getState().nativeNavigationSyncs["/workspace"]).toBeUndefined()
    })
    expect(ipcMocks.previewBack).toHaveBeenCalledTimes(1)
    expect(ipcMocks.previewOpenUrl).not.toHaveBeenCalled()

    fireEvent.click(screen.getByRole("button", { name: i18n.t("previewPanel.forward", { ns: "panels" }) }))
    await waitFor(() => {
      expect(usePreviewStore.getState().navForWorkspace("/workspace").url)
        .toBe("https://example.com/b")
      expect(usePreviewStore.getState().nativeNavigationSyncs["/workspace"]).toBeUndefined()
    })
    expect(ipcMocks.previewForward).toHaveBeenCalledTimes(1)
    expect(ipcMocks.previewOpenUrl).not.toHaveBeenCalled()
  })

  it("preserves A→B→local history continuity with deterministic external fallbacks", async () => {
    ;(globalThis as { isTauri?: boolean }).isTauri = true
    const preview = usePreviewStore.getState()
    preview.navigate("/workspace", "https://example.com/a")
    render(<PreviewPanel />)
    await waitFor(() => expect(usePreviewStore.getState().nativeSession?.currentUrl)
      .toBe("https://example.com/a"))
    preview.navigate("/workspace", "https://example.com/b")
    await waitFor(() => expect(usePreviewStore.getState().nativeSession?.currentUrl)
      .toBe("https://example.com/b"))
    preview.navigate("/workspace", "http://localhost:8765/")
    await waitFor(() => expect(screen.getByTitle("Live preview")).toBeInTheDocument())
    expect(usePreviewStore.getState().nativeSession).toBeNull()
    ipcMocks.previewBack.mockClear()
    ipcMocks.previewForward.mockClear()
    ipcMocks.previewOpenUrl.mockClear()

    fireEvent.click(screen.getByRole("button", { name: i18n.t("previewPanel.back", { ns: "panels" }) }))
    await waitFor(() => expect(usePreviewStore.getState().nativeSession?.currentUrl)
      .toBe("https://example.com/b"))
    fireEvent.click(screen.getByRole("button", { name: i18n.t("previewPanel.back", { ns: "panels" }) }))
    await waitFor(() => expect(usePreviewStore.getState().nativeSession?.currentUrl)
      .toBe("https://example.com/a"))
    expect(ipcMocks.previewBack).not.toHaveBeenCalled()

    fireEvent.click(screen.getByRole("button", { name: i18n.t("previewPanel.forward", { ns: "panels" }) }))
    await waitFor(() => expect(usePreviewStore.getState().nativeSession?.currentUrl)
      .toBe("https://example.com/b"))
    expect(ipcMocks.previewForward).not.toHaveBeenCalled()
    fireEvent.click(screen.getByRole("button", { name: i18n.t("previewPanel.forward", { ns: "panels" }) }))
    await waitFor(() => expect(screen.getByTitle("Live preview")).toHaveAttribute(
      "src",
      "http://localhost:8765/"
    ))
    expect(usePreviewStore.getState().nativeSession).toBeNull()
  })

  it("invalidates native continuity across PreviewPanel unmount/remount", async () => {
    ;(globalThis as { isTauri?: boolean }).isTauri = true
    const preview = usePreviewStore.getState()
    preview.navigate("/workspace", "https://example.com/a")
    const first = render(<PreviewPanel />)
    await waitFor(() => expect(usePreviewStore.getState().nativeSession?.currentUrl)
      .toBe("https://example.com/a"))
    preview.navigate("/workspace", "https://example.com/b")
    await waitFor(() => expect(usePreviewStore.getState().nativeSession?.currentUrl)
      .toBe("https://example.com/b"))
    first.unmount()
    await waitFor(() => expect(usePreviewStore.getState().nativeSession).toBeNull())

    render(<PreviewPanel />)
    await waitFor(() => expect(usePreviewStore.getState().nativeSession).toMatchObject({
      currentUrl: "https://example.com/b",
      backStack: [],
    }))
    ipcMocks.previewBack.mockClear()
    ipcMocks.previewOpenUrl.mockClear()
    fireEvent.click(screen.getByRole("button", { name: i18n.t("previewPanel.back", { ns: "panels" }) }))
    await waitFor(() => expect(usePreviewStore.getState().nativeSession?.currentUrl)
      .toBe("https://example.com/a"))
    expect(ipcMocks.previewBack).not.toHaveBeenCalled()
    expect(ipcMocks.previewOpenUrl).toHaveBeenCalledWith("https://example.com/a", 0, 0, 0, 0)
  })

  it("invalidates the previous owner when another workspace switches to local preview", async () => {
    ;(globalThis as { isTauri?: boolean }).isTauri = true
    const preview = usePreviewStore.getState()
    preview.navigate("/workspace", "https://example.com/a")
    render(<PreviewPanel />)
    await waitFor(() => expect(usePreviewStore.getState().nativeSession).toMatchObject({
      workspacePath: "/workspace",
      currentUrl: "https://example.com/a",
    }))

    preview.navigate("/workspace-b", "http://localhost:8765/")
    useWorkspaceStore.setState({ workspacePath: "/workspace-b" })

    await waitFor(() => expect(usePreviewStore.getState().nativeSession).toBeNull())
    expect(ipcMocks.previewClose).toHaveBeenCalled()

    useWorkspaceStore.setState({ workspacePath: "/workspace" })
    await waitFor(() => expect(usePreviewStore.getState().nativeSession).toMatchObject({
      workspacePath: "/workspace",
      currentUrl: "https://example.com/a",
      backStack: [],
    }))
  })

  it("serializes deferred A/B opens so the newest URL owns the singleton ledger", async () => {
    ;(globalThis as { isTauri?: boolean }).isTauri = true
    const openA = deferred<void>()
    const openB = deferred<void>()
    ipcMocks.previewOpenUrl
      .mockImplementationOnce(() => openA.promise)
      .mockImplementationOnce(() => openB.promise)
    const preview = usePreviewStore.getState()
    preview.navigate("/workspace", "https://example.com/a")
    render(<PreviewPanel />)
    await waitFor(() => expect(ipcMocks.previewOpenUrl).toHaveBeenCalledTimes(1))

    preview.navigate("/workspace", "https://example.com/b")
    await waitFor(() => expect(usePreviewStore.getState().nativeRequest).toMatchObject({
      kind: "open",
      url: "https://example.com/b",
    }))
    expect(ipcMocks.previewOpenUrl).toHaveBeenCalledTimes(1)

    openA.resolve(undefined)
    await waitFor(() => expect(ipcMocks.previewOpenUrl).toHaveBeenCalledTimes(2))
    openB.resolve(undefined)
    await waitFor(() => expect(usePreviewStore.getState().nativeSession).toMatchObject({
      workspacePath: "/workspace",
      currentUrl: "https://example.com/b",
    }))
  })

  it("queues Reload behind a pending external open", async () => {
    ;(globalThis as { isTauri?: boolean }).isTauri = true
    const opening = deferred<void>()
    ipcMocks.previewOpenUrl.mockImplementationOnce(() => opening.promise)
    usePreviewStore.getState().navigate("/workspace", "https://example.com/pending")
    render(<PreviewPanel />)
    await waitFor(() => expect(ipcMocks.previewOpenUrl).toHaveBeenCalledTimes(1))

    fireEvent.click(screen.getByRole("button", { name: i18n.t("previewPanel.reload", { ns: "panels" }) }))
    await new Promise((resolve) => setTimeout(resolve, 0))
    expect(ipcMocks.previewReload).not.toHaveBeenCalled()

    opening.resolve(undefined)
    await waitFor(() => expect(ipcMocks.previewReload).toHaveBeenCalledTimes(1))
    expect(usePreviewStore.getState().nativeRequest).toBeNull()
    expect(usePreviewStore.getState().nativeSession?.currentUrl)
      .toBe("https://example.com/pending")
  })

  it("queues Back behind pending B and revalidates the established native history", async () => {
    ;(globalThis as { isTauri?: boolean }).isTauri = true
    const preview = usePreviewStore.getState()
    preview.navigate("/workspace", "https://example.com/a")
    render(<PreviewPanel />)
    await waitFor(() => expect(usePreviewStore.getState().nativeSession?.currentUrl)
      .toBe("https://example.com/a"))

    const openingB = deferred<void>()
    ipcMocks.previewOpenUrl.mockImplementationOnce(() => openingB.promise)
    preview.navigate("/workspace", "https://example.com/b")
    await waitFor(() => expect(usePreviewStore.getState().nativeRequest).toMatchObject({
      kind: "open",
      url: "https://example.com/b",
    }))
    fireEvent.click(screen.getByRole("button", { name: i18n.t("previewPanel.back", { ns: "panels" }) }))
    await new Promise((resolve) => setTimeout(resolve, 0))
    expect(ipcMocks.previewBack).not.toHaveBeenCalled()

    openingB.resolve(undefined)
    await waitFor(() => expect(ipcMocks.previewBack).toHaveBeenCalledTimes(1))
    await waitFor(() => expect(usePreviewStore.getState().navForWorkspace("/workspace").url)
      .toBe("https://example.com/a"))
  })

  it("closes unknown native content when the newest open fails after an older success", async () => {
    ;(globalThis as { isTauri?: boolean }).isTauri = true
    const openA = deferred<void>()
    ipcMocks.previewOpenUrl
      .mockImplementationOnce(() => openA.promise)
      .mockRejectedValueOnce(new Error("B failed"))
    const preview = usePreviewStore.getState()
    preview.navigate("/workspace", "https://example.com/a")
    render(<PreviewPanel />)
    await waitFor(() => expect(ipcMocks.previewOpenUrl).toHaveBeenCalledTimes(1))
    preview.navigate("/workspace", "https://example.com/b")
    await waitFor(() => expect(usePreviewStore.getState().nativeRequest).toMatchObject({
      kind: "open",
      url: "https://example.com/b",
    }))
    fireEvent.click(screen.getByRole("button", { name: i18n.t("previewPanel.reload", { ns: "panels" }) }))

    openA.resolve(undefined)
    await waitFor(() => expect(ipcMocks.showActionError).toHaveBeenCalledWith(
      i18n.t("previewPanel.reload", { ns: "panels" }),
      expect.objectContaining({ message: "B failed" })
    ))
    expect(ipcMocks.previewClose).toHaveBeenCalled()
    expect(usePreviewStore.getState().nativeSession).toBeNull()
    expect(usePreviewStore.getState().navForWorkspace("/workspace").url)
      .toBe("https://example.com/b")
    expect(ipcMocks.previewReload).not.toHaveBeenCalled()
  })

  it("does not let stale unmount cleanup close a newer workspace owner", async () => {
    ;(globalThis as { isTauri?: boolean }).isTauri = true
    const openA = deferred<void>()
    const openB = deferred<void>()
    ipcMocks.previewOpenUrl
      .mockImplementationOnce(() => openA.promise)
      .mockImplementationOnce(() => openB.promise)
    const preview = usePreviewStore.getState()
    preview.navigate("/workspace", "https://example.com/a")
    const oldPanel = render(<PreviewPanel />)
    await waitFor(() => expect(ipcMocks.previewOpenUrl).toHaveBeenCalledTimes(1))
    oldPanel.unmount()

    useWorkspaceStore.setState({ workspacePath: "/other" })
    preview.navigate("/other", "https://example.com/b")
    render(<PreviewPanel />)
    await waitFor(() => expect(usePreviewStore.getState().nativeRequest).toMatchObject({
      kind: "open",
      workspacePath: "/other",
      url: "https://example.com/b",
    }))
    openA.resolve(undefined)
    await waitFor(() => expect(ipcMocks.previewOpenUrl).toHaveBeenCalledTimes(2))
    openB.resolve(undefined)
    await waitFor(() => expect(usePreviewStore.getState().nativeSession).toMatchObject({
      workspacePath: "/other",
      currentUrl: "https://example.com/b",
    }))
    expect(ipcMocks.previewClose).not.toHaveBeenCalled()
  })

  it("surfaces opener, native navigation and stop failures while preserving running state", async () => {
    const preview = usePreviewStore.getState()
    preview.navigate("/workspace", "https://example.com/first")
    preview.recordNativeOpen("/workspace", "https://example.com/first")
    preview.navigate("/workspace", "https://example.com/second")
    preview.recordNativeOpen("/workspace", "https://example.com/second")
    preview.setDevServer({
      workspace: "/workspace",
      command: "bun run dev",
      port: null,
      status: { status: "running", port: null },
    })
    ipcMocks.previewReload.mockRejectedValueOnce(new Error("native reload failed"))
    ipcMocks.openUrl.mockRejectedValueOnce(new Error("opener failed"))
    ipcMocks.devServerStop.mockRejectedValueOnce(new Error("stop failed"))
    render(<PreviewPanel />)

    fireEvent.click(screen.getByRole("button", { name: i18n.t("previewPanel.reload", { ns: "panels" }) }))
    fireEvent.click(screen.getByRole("button", { name: i18n.t("previewPanel.openExternally", { ns: "panels" }) }))
    fireEvent.click(screen.getByRole("button", { name: i18n.t("previewPanel.stop", { ns: "panels" }) }))

    await waitFor(() => expect(ipcMocks.showActionError).toHaveBeenCalledTimes(3))
    expect(ipcMocks.showActionError).toHaveBeenCalledWith(
      i18n.t("previewPanel.reload", { ns: "panels" }),
      expect.objectContaining({ message: "native reload failed" })
    )
    expect(usePreviewStore.getState().devServerForWorkspace("/workspace")?.status.status)
      .toBe("running")
    expect(usePreviewStore.getState().attemptForWorkspace("/workspace")).toBe(0)
  })
})

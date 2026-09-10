import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"
import { act, cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react"

import { PreviewPanel } from "@/app/panels/PreviewPanel"
import i18n from "@/lib/i18n"
import { useAppDialogStore } from "@/state/appDialogStore"
import { useContextMenuStore } from "@/state/contextMenuStore"
import { useTextInputDialogStore } from "@/state/textInputDialogStore"
import { usePreviewStore } from "@/state/previewStore"
import { useWorkspaceStore } from "@/state/workspaceStore"

// This file focuses on coverage panels.test.tsx's "PreviewPanel dev server flow"
// describe block doesn't already have: the no-candidates branch, an IPC-rejection
// error path, and the native child-webview lifecycle (open/bounds/visible/close)
// gated by isTauri(). Basic detect->start and stop flows are included too, kept
// deliberately small since the happy paths are already covered there.

const ipcMocks = vi.hoisted(() => ({
  previewNavigationState: vi.fn(),
  requestDevServerAuthorization: vi.fn(),
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

vi.mock("@/state/workspaceTrustStore", async (importOriginal) => ({
  ...(await importOriginal<typeof import("@/state/workspaceTrustStore")>()),
  requestDevServerAuthorization: (...args: unknown[]) =>
    ipcMocks.requestDevServerAuthorization(...args),
}))

vi.mock("@/lib/ipc", async (importOriginal) => ({
  ...(await importOriginal<typeof import("@/lib/ipc")>()),
  previewNavigationState: (...args: unknown[]) => ipcMocks.previewNavigationState(...args),
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
  useAppDialogStore.setState({ pending: null })
  useContextMenuStore.setState({ request: null, x: 0, y: 0, availabilityRevision: 0 })
  ipcMocks.requestDevServerAuthorization.mockResolvedValue("challenge-1")
  ipcMocks.previewBack.mockImplementation(async () => { usePreviewStore.getState().syncNativeBack("/workspace") })
  ipcMocks.previewForward.mockImplementation(async () => { usePreviewStore.getState().syncNativeForward("/workspace") })
  ipcMocks.previewNavigationState.mockImplementation(async () => {
    const session = usePreviewStore.getState().nativeSession!
    return { sessionId: session.sessionId, url: session.currentUrl,
      canGoBack: session.backStack.length > 0, canGoForward: session.forwardStack.length > 0 }
  })
})

afterEach(async () => {
  cleanup()
  await new Promise((resolve) => setTimeout(resolve, 0))
  usePreviewStore.getState().reset()
  useWorkspaceStore.setState({ workspacePath: null })
  useAppDialogStore.setState({ pending: null })
  useContextMenuStore.setState({ request: null })
  useTextInputDialogStore.setState({ pending: null })
  delete (globalThis as { isTauri?: boolean }).isTauri
  vi.clearAllMocks()
})

describe("PreviewPanel", () => {

  it.each([
    ["localhost:5173", "http://localhost:5173"],
    ["127.0.0.1:4173/path", "http://127.0.0.1:4173/path"],
    ["devbox:3000", "https://devbox:3000"],
    ["example.com:8080/docs", "https://example.com:8080/docs"],
    ["http://example.com:8080", "http://example.com:8080"],
    ["https://example.com:8080", "https://example.com:8080"],
  ])("normalizes %s to %s", (inputUrl, expectedUrl) => {
    render(<PreviewPanel />)
    const input = screen.getByLabelText(i18n.t("previewPanel.urlLabel", { ns: "panels" }))

    fireEvent.change(input, { target: { value: inputUrl } })
    fireEvent.keyDown(input, { key: "Enter" })

    expect(usePreviewStore.getState().navForWorkspace("/workspace").url).toBe(expectedUrl)
    expect(screen.queryByRole("alert")).not.toBeInTheDocument()
  })

  it.each(["ftp://example.com", "file:///etc/passwd", "javascript:alert(1)"])(
    "keeps %s out of navigation and exposes an accessible error",
    async (unsafeUrl) => {
      usePreviewStore.getState().navigate("/workspace", "http://localhost:5173")
      render(<PreviewPanel />)
      const input = screen.getByLabelText(i18n.t("previewPanel.urlLabel", { ns: "panels" }))

      fireEvent.change(input, { target: { value: unsafeUrl } })
      fireEvent.keyDown(input, { key: "Enter" })

      expect(await screen.findByRole("alert")).toHaveTextContent(
        "Browser URLs must use http:// or https://"
      )
      expect(usePreviewStore.getState().navForWorkspace("/workspace").url)
        .toBe("http://localhost:5173")
    }
  )
})

describe("PreviewPanel native child-webview lifecycle (Tauri only)", () => {
  it("opens the webview, syncs bounds/visibility on mount, and closes it on unmount", async () => {
    ;(globalThis as { isTauri?: boolean }).isTauri = true
    usePreviewStore.getState().navigate("/workspace", "https://example.com")

    const { unmount } = render(<PreviewPanel />)

    await waitFor(() =>
      expect(ipcMocks.previewOpenUrl).toHaveBeenCalledWith("https://example.com", 0, 0, 0, 0, expect.any(String))
    )
    expect(ipcMocks.previewSetBounds).toHaveBeenCalledWith(0, 0, 0, 0)
    expect(ipcMocks.previewSetVisible).toHaveBeenCalledWith(true)
    expect(ipcMocks.previewClose).not.toHaveBeenCalled()

    unmount()
    await waitFor(() => expect(ipcMocks.previewClose).toHaveBeenCalled())
  })

  it.each(["http://127.0.0.1:34329", "https://example.com"])("QA26-006 reads actual native URL and history for %s", async (origin) => {
    ;(globalThis as { isTauri?: boolean }).isTauri = true
    let browser = { url: `${origin}/`, canGoBack: false, canGoForward: false }
    ipcMocks.previewNavigationState.mockImplementation(async (sessionId: string) => ({ sessionId, ...browser }))
    ipcMocks.previewBack.mockImplementationOnce(async () => { browser = { url: `${origin}/`, canGoBack: false, canGoForward: true } })
    ipcMocks.previewForward.mockImplementationOnce(async () => { browser = { url: `${origin}/next`, canGoBack: true, canGoForward: false } })
    usePreviewStore.getState().navigate("/workspace", origin)
    render(<PreviewPanel />)
    await waitFor(() => expect(ipcMocks.previewNavigationState).toHaveBeenCalled())
    expect(screen.getByRole("button", { name: i18n.t("previewPanel.back", { ns: "panels" }) })).toBeDisabled()
    browser = { url: `${origin}/next`, canGoBack: true, canGoForward: false }
    await waitFor(() => expect(screen.getByLabelText(i18n.t("previewPanel.urlLabel", { ns: "panels" })))
      .toHaveValue(`${origin}/next`))
    expect(ipcMocks.previewOpenUrl).toHaveBeenCalledTimes(1)
    fireEvent.click(screen.getByRole("button", { name: i18n.t("previewPanel.back", { ns: "panels" }) }))
    await waitFor(() => expect(screen.getByLabelText(i18n.t("previewPanel.urlLabel", { ns: "panels" }))).toHaveValue(`${origin}/`))
    expect(ipcMocks.previewBack).toHaveBeenCalledTimes(1)
    fireEvent.click(screen.getByRole("button", { name: i18n.t("previewPanel.forward", { ns: "panels" }) }))
    await waitFor(() => expect(screen.getByLabelText(i18n.t("previewPanel.urlLabel", { ns: "panels" }))).toHaveValue(`${origin}/next`))
    expect(ipcMocks.previewForward).toHaveBeenCalledTimes(1)
    expect(ipcMocks.previewOpenUrl).toHaveBeenCalledTimes(1)
  })

  it("keeps snapshot polling single-flight and discards results after an overlay hides the owner", async () => {
    ;(globalThis as { isTauri?: boolean }).isTauri = true
    const pending = deferred<{ sessionId: string; url: string; canGoBack: boolean; canGoForward: boolean }>()
    ipcMocks.previewNavigationState.mockImplementationOnce(() => pending.promise)
    usePreviewStore.getState().navigate("/workspace", "https://example.com/")
    const { unmount } = render(<PreviewPanel />)
    await waitFor(() => expect(ipcMocks.previewNavigationState).toHaveBeenCalledTimes(1))
    const sessionId = ipcMocks.previewNavigationState.mock.calls[0][0] as string
    await act(async () => { await new Promise((resolve) => setTimeout(resolve, 300)) })
    expect(ipcMocks.previewNavigationState).toHaveBeenCalledTimes(1)
    act(() => useAppDialogStore.setState({ pending: { type: "message", title: "Modal", description: "Modal", resolve: () => {} } }))
    await act(async () => pending.resolve({ sessionId, url: "https://example.com/stale", canGoBack: true, canGoForward: false }))
    expect(usePreviewStore.getState().navForWorkspace("/workspace").url).toBe("https://example.com/")
    await act(async () => { await new Promise((resolve) => setTimeout(resolve, 300)) })
    expect(ipcMocks.previewNavigationState).toHaveBeenCalledTimes(1)
    act(() => useAppDialogStore.setState({ pending: null }))
    await waitFor(() => expect(ipcMocks.previewNavigationState).toHaveBeenCalledTimes(2))
    unmount()
    await act(async () => { await new Promise((resolve) => setTimeout(resolve, 300)) })
    expect(ipcMocks.previewNavigationState).toHaveBeenCalledTimes(2)
  })

  it("discards a snapshot if a newer open generation started while it was in flight", async () => {
    ;(globalThis as { isTauri?: boolean }).isTauri = true
    const pending = deferred<{ sessionId: string; url: string; canGoBack: boolean; canGoForward: boolean }>()
    ipcMocks.previewNavigationState.mockImplementationOnce(() => pending.promise)
    usePreviewStore.getState().navigate("/workspace", "https://example.com/")
    render(<PreviewPanel />)
    await waitFor(() => expect(ipcMocks.previewNavigationState).toHaveBeenCalledTimes(1))
    const sessionId = ipcMocks.previewNavigationState.mock.calls[0][0] as string
    const state = usePreviewStore.getState()
    const token = state.beginNativeOpenRequest("/workspace", "https://example.com/")
    state.settleNativeRequest(token)
    await act(async () => pending.resolve({ sessionId, url: "https://example.com/stale", canGoBack: true, canGoForward: false }))
    expect(usePreviewStore.getState().navForWorkspace("/workspace").url).toBe("https://example.com/")
  })

  it("serializes visibility behind a pending open and revalidates the latest overlay state", async () => {
    ;(globalThis as { isTauri?: boolean }).isTauri = true
    const opening = deferred<void>()
    ipcMocks.previewOpenUrl.mockImplementationOnce(() => opening.promise)
    usePreviewStore.getState().navigate("/workspace", "https://example.com")
    render(<PreviewPanel />)
    await waitFor(() => expect(ipcMocks.previewOpenUrl).toHaveBeenCalledTimes(1))

    useAppDialogStore.setState({
      pending: {
        type: "message",
        title: "Drop failed",
        description: "Is a directory",
        resolve: () => {},
      },
    })

    expect(ipcMocks.previewSetVisible).not.toHaveBeenCalled()
    opening.resolve(undefined)
    await waitFor(() => expect(ipcMocks.previewSetVisible).toHaveBeenCalledWith(false))
    expect(ipcMocks.previewSetVisible).not.toHaveBeenCalledWith(true)
  })

  it("does not reopen an external preview when the app language changes", async () => {
    ;(globalThis as { isTauri?: boolean }).isTauri = true
    const originalLanguage = i18n.resolvedLanguage ?? i18n.language
    const nextLanguage = originalLanguage === "zh-TW" ? "en" : "zh-TW"
    usePreviewStore.getState().navigate("/workspace", "https://example.com")
    const { unmount } = render(<PreviewPanel />)

    try {
      await waitFor(() => expect(ipcMocks.previewOpenUrl).toHaveBeenCalledTimes(1))
      await waitFor(() => expect(usePreviewStore.getState().nativeSession?.currentUrl)
        .toBe("https://example.com"))

      await act(async () => {
        await i18n.changeLanguage(nextLanguage)
        await new Promise((resolve) => setTimeout(resolve, 0))
      })

      expect(screen.getByRole("button", {
        name: i18n.t("previewPanel.reload", { ns: "panels" }),
      })).toBeInTheDocument()
      expect(ipcMocks.previewOpenUrl).toHaveBeenCalledTimes(1)
    } finally {
      unmount()
      await act(async () => {
        await i18n.changeLanguage(originalLanguage)
      })
    }
  })

  it("uses the latest language when a pending native open reports an error", async () => {
    ;(globalThis as { isTauri?: boolean }).isTauri = true
    const originalLanguage = i18n.resolvedLanguage ?? i18n.language
    const nextLanguage = originalLanguage === "zh-TW" ? "en" : "zh-TW"
    const opening = deferred<void>()
    ipcMocks.previewOpenUrl.mockImplementationOnce(() => opening.promise)
    usePreviewStore.getState().navigate("/workspace", "https://example.com")
    const { unmount } = render(<PreviewPanel />)

    try {
      await waitFor(() => expect(ipcMocks.previewOpenUrl).toHaveBeenCalledTimes(1))
      await act(async () => {
        await i18n.changeLanguage(nextLanguage)
      })
      opening.reject(new Error("native open failed"))

      await waitFor(() => expect(ipcMocks.showActionError).toHaveBeenCalledWith(
        i18n.t("previewPanel.reload", { ns: "panels" }),
        expect.objectContaining({ message: "native open failed" })
      ))
      expect(ipcMocks.previewOpenUrl).toHaveBeenCalledTimes(1)
    } finally {
      unmount()
      await act(async () => {
        await i18n.changeLanguage(originalLanguage)
      })
    }
  })

  it("hides the native webview while an app-owned message dialog is open", async () => {
    ;(globalThis as { isTauri?: boolean }).isTauri = true
    usePreviewStore.getState().navigate("/workspace", "https://example.com")
    render(<PreviewPanel />)
    await waitFor(() => expect(ipcMocks.previewSetVisible).toHaveBeenCalledWith(true))

    useAppDialogStore.setState({
      pending: {
        type: "message",
        title: "Drop failed",
        description: "Is a directory",
        resolve: () => {},
      },
    })

    await waitFor(() => expect(ipcMocks.previewSetVisible).toHaveBeenCalledWith(false))
  })

  it("hides the native webview while a text-input dialog is open", async () => {
    ;(globalThis as { isTauri?: boolean }).isTauri = true
    usePreviewStore.getState().navigate("/workspace", "https://example.com")
    render(<PreviewPanel />)
    await waitFor(() => expect(ipcMocks.previewSetVisible).toHaveBeenCalledWith(true))

    useTextInputDialogStore.setState({
      pending: {
        requestId: 1,
        title: "Create branch from tag",
        label: "Branch name",
        confirmLabel: "Create branch",
        resolve: () => {},
      },
    })

    await waitFor(() => expect(ipcMocks.previewSetVisible).toHaveBeenCalledWith(false))
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

  it("does not attach the Yuzora preview menu to iframe/frame shell or native host", () => {
    usePreviewStore.getState().navigate("/workspace", "http://localhost:5173")
    const { unmount } = render(<PreviewPanel />)

    fireEvent.contextMenu(screen.getByTestId("preview-frame-shell"))
    expect(useContextMenuStore.getState().request).toBeNull()
    fireEvent.contextMenu(screen.getByTitle("Browser"))
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
    expect(screen.getByTitle("Browser")).toHaveAttribute("src", "http://localhost:8765/")
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

    await waitFor(() => expect(screen.getByRole("button", { name: i18n.t("previewPanel.back", { ns: "panels" }) })).toBeEnabled())
    fireEvent.click(screen.getByRole("button", { name: i18n.t("previewPanel.back", { ns: "panels" }) }))
    await waitFor(() => {
      expect(usePreviewStore.getState().navForWorkspace("/workspace").url)
        .toBe("https://example.com/a")
      expect(usePreviewStore.getState().nativeNavigationSyncs["/workspace"]).toBeUndefined()
    })
    expect(ipcMocks.previewBack).toHaveBeenCalledTimes(1)
    expect(ipcMocks.previewOpenUrl).not.toHaveBeenCalled()

    await waitFor(() => expect(screen.getByRole("button", { name: i18n.t("previewPanel.forward", { ns: "panels" }) })).toBeEnabled())
    fireEvent.click(screen.getByRole("button", { name: i18n.t("previewPanel.forward", { ns: "panels" }) }))
    await waitFor(() => {
      expect(usePreviewStore.getState().navForWorkspace("/workspace").url)
        .toBe("https://example.com/b")
      expect(usePreviewStore.getState().nativeNavigationSyncs["/workspace"]).toBeUndefined()
    })
    expect(ipcMocks.previewForward).toHaveBeenCalledTimes(1)
    expect(ipcMocks.previewOpenUrl).not.toHaveBeenCalled()
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
    expect(ipcMocks.previewOpenUrl).toHaveBeenCalledWith("https://example.com/a", 0, 0, 0, 0, expect.any(String))
  })

  it("invalidates the previous owner when another workspace switches to native local preview", async () => {
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

    await waitFor(() => expect(usePreviewStore.getState().nativeSession?.workspacePath).toBe("/workspace-b"))
    expect(ipcMocks.previewOpenUrl.mock.calls[1][5]).not.toBe(ipcMocks.previewOpenUrl.mock.calls[0][5])

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

  it("disables Back while the newest URL is still opening", async () => {
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
    await waitFor(() => expect(usePreviewStore.getState().nativeRequest).toBeNull())
    expect(ipcMocks.previewBack).not.toHaveBeenCalled()
    expect(usePreviewStore.getState().navForWorkspace("/workspace").url).toBe("https://example.com/b")
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
})

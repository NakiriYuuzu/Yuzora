import { beforeEach, describe, expect, it, vi } from "vitest"

import {
  copyPreviewUrl,
  enqueueNativePreviewOperation,
  goBackPreview,
  goForwardPreview,
  openPreviewExternally,
  previewTargetCanGoBack,
  previewTargetCanGoForward,
  previewTargetHasRunningServer,
  reloadPreview,
  stopPreviewDevServer,
  type PreviewCommandTarget,
} from "@/preview/previewCommands"
import { usePreviewStore } from "@/state/previewStore"
import { useWorkspaceStore } from "@/state/workspaceStore"

const commandMocks = vi.hoisted(() => ({
  writeText: vi.fn(),
  openUrl: vi.fn(),
  devServerStop: vi.fn(),
  previewBack: vi.fn(),
  previewForward: vi.fn(),
  previewReload: vi.fn(),
}))

vi.mock("@tauri-apps/plugin-clipboard-manager", () => ({
  writeText: (...args: unknown[]) => commandMocks.writeText(...args),
}))

vi.mock("@tauri-apps/plugin-opener", () => ({
  openUrl: (...args: unknown[]) => commandMocks.openUrl(...args),
}))

vi.mock("@/lib/ipc", async (importOriginal) => ({
  ...(await importOriginal<typeof import("@/lib/ipc")>()),
  devServerStop: (...args: unknown[]) => commandMocks.devServerStop(...args),
  previewBack: (...args: unknown[]) => commandMocks.previewBack(...args),
  previewForward: (...args: unknown[]) => commandMocks.previewForward(...args),
  previewReload: (...args: unknown[]) => commandMocks.previewReload(...args),
}))

function target(workspacePath = "/ws/a"): PreviewCommandTarget {
  const preview = usePreviewStore.getState()
  return {
    workspacePath,
    url: preview.navForWorkspace(workspacePath).url,
    serverAttempt: preview.attemptForWorkspace(workspacePath),
  }
}

beforeEach(() => {
  vi.clearAllMocks()
  usePreviewStore.getState().reset()
  useWorkspaceStore.setState({ workspacePath: "/ws/a" })
})

describe("previewCommands", () => {
  it("routes local Back, Forward and Reload through the workspace-scoped store", async () => {
    const preview = usePreviewStore.getState()
    preview.navigate("/ws/a", "http://localhost:5173")
    preview.navigate("/ws/a", "http://localhost:5173/about")
    const backTarget = target()

    expect(previewTargetCanGoBack(backTarget)).toBe(true)
    expect(await goBackPreview(backTarget)).toBe("completed")
    expect(usePreviewStore.getState().navForWorkspace("/ws/a").url)
      .toBe("http://localhost:5173")

    const forwardTarget = target()
    expect(previewTargetCanGoForward(forwardTarget)).toBe(true)
    expect(await goForwardPreview(forwardTarget)).toBe("completed")
    expect(usePreviewStore.getState().navForWorkspace("/ws/a").url)
      .toBe("http://localhost:5173/about")

    const reloadTarget = target()
    expect(await reloadPreview(reloadTarget)).toBe("completed")
    expect(usePreviewStore.getState().navForWorkspace("/ws/a").reloadNonce).toBe(1)
    expect(commandMocks.previewBack).not.toHaveBeenCalled()
    expect(commandMocks.previewForward).not.toHaveBeenCalled()
    expect(commandMocks.previewReload).not.toHaveBeenCalled()
  })

  it("routes external-to-external Back/Forward natively and synchronizes URL/stacks", async () => {
    const preview = usePreviewStore.getState()
    preview.navigate("/ws/a", "https://example.com/first")
    preview.recordNativeOpen("/ws/a", "https://example.com/first")
    preview.navigate("/ws/a", "https://example.com/second")
    preview.recordNativeOpen("/ws/a", "https://example.com/second")

    expect(await goBackPreview(target())).toBe("completed")
    expect(commandMocks.previewBack).toHaveBeenCalledTimes(1)
    expect(usePreviewStore.getState().navForWorkspace("/ws/a")).toMatchObject({
      url: "https://example.com/first",
      backStack: [],
      forwardStack: ["https://example.com/second"],
    })
    expect(previewTargetCanGoBack(target())).toBe(false)
    expect(previewTargetCanGoForward(target())).toBe(true)

    expect(await goForwardPreview(target())).toBe("completed")
    expect(commandMocks.previewForward).toHaveBeenCalledTimes(1)
    expect(usePreviewStore.getState().navForWorkspace("/ws/a")).toMatchObject({
      url: "https://example.com/second",
      backStack: ["https://example.com/first"],
      forwardStack: [],
    })
    expect(await reloadPreview(target())).toBe("completed")
    expect(commandMocks.previewReload).toHaveBeenCalledTimes(1)
    expect(usePreviewStore.getState().navForWorkspace("/ws/a").reloadNonce).toBe(0)
  })

  it("queues Forward behind a pending native open and revalidates before IPC", async () => {
    let resolveOpen!: () => void
    const openGate = new Promise<void>((resolve) => {
      resolveOpen = resolve
    })
    const preview = usePreviewStore.getState()
    preview.navigate("/ws/a", "https://example.com/a")
    preview.recordNativeOpen("/ws/a", "https://example.com/a")
    preview.navigate("/ws/a", "https://example.com/b")
    preview.recordNativeOpen("/ws/a", "https://example.com/b")
    expect(preview.syncNativeBack("/ws/a")).toBe(true)
    const marker = usePreviewStore.getState().nativeNavigationSyncs["/ws/a"]
    if (!marker) throw new Error("missing native navigation marker")
    preview.consumeNativeNavigationSync("/ws/a", marker.token)

    const requestToken = preview.beginNativeOpenRequest("/ws/a", "https://example.com/a")
    const opening = enqueueNativePreviewOperation(async () => {
      await openGate
      const state = usePreviewStore.getState()
      if (!state.nativeRequestIsCurrent(requestToken)) return
      state.recordNativeOpen("/ws/a", "https://example.com/a")
      state.settleNativeRequest(requestToken)
    })
    const forwarding = goForwardPreview(target())
    await Promise.resolve()
    expect(commandMocks.previewForward).not.toHaveBeenCalled()

    resolveOpen()
    await opening
    await expect(forwarding).resolves.toBe("completed")
    expect(commandMocks.previewForward).toHaveBeenCalledTimes(1)
    expect(usePreviewStore.getState().navForWorkspace("/ws/a").url)
      .toBe("https://example.com/b")
  })

  it("switches renderer directly when external history crosses the local boundary", async () => {
    const preview = usePreviewStore.getState()
    preview.navigate("/ws/a", "http://localhost:5173")
    preview.navigate("/ws/a", "https://example.com")

    expect(await goBackPreview(target())).toBe("completed")
    expect(usePreviewStore.getState().navForWorkspace("/ws/a").url)
      .toBe("http://localhost:5173")
    expect(commandMocks.previewBack).not.toHaveBeenCalled()
    expect(previewTargetCanGoForward(target())).toBe(true)

    expect(await goForwardPreview(target())).toBe("completed")
    expect(usePreviewStore.getState().navForWorkspace("/ws/a").url)
      .toBe("https://example.com")
    expect(commandMocks.previewForward).not.toHaveBeenCalled()
    expect(previewTargetCanGoBack(target())).toBe(true)
  })

  it("falls back deterministically after local rendering breaks native continuity", async () => {
    const preview = usePreviewStore.getState()
    preview.navigate("/ws/a", "https://example.com/a")
    preview.recordNativeOpen("/ws/a", "https://example.com/a")
    preview.navigate("/ws/a", "https://example.com/b")
    preview.recordNativeOpen("/ws/a", "https://example.com/b")
    preview.navigate("/ws/a", "http://localhost:5173")
    preview.closeNativeSession("/ws/a")

    expect(await goBackPreview(target())).toBe("completed")
    expect(usePreviewStore.getState().navForWorkspace("/ws/a").url)
      .toBe("https://example.com/b")
    preview.recordNativeOpen("/ws/a", "https://example.com/b")

    expect(await goBackPreview(target())).toBe("completed")
    expect(usePreviewStore.getState().navForWorkspace("/ws/a").url)
      .toBe("https://example.com/a")
    expect(commandMocks.previewBack).not.toHaveBeenCalled()
    preview.recordNativeOpen("/ws/a", "https://example.com/a")

    expect(await goForwardPreview(target())).toBe("completed")
    expect(usePreviewStore.getState().navForWorkspace("/ws/a").url)
      .toBe("https://example.com/b")
    expect(commandMocks.previewForward).not.toHaveBeenCalled()
    preview.recordNativeOpen("/ws/a", "https://example.com/b")

    expect(await goForwardPreview(target())).toBe("completed")
    expect(usePreviewStore.getState().navForWorkspace("/ws/a").url)
      .toBe("http://localhost:5173")
  })

  it("copies and opens only the snapshotted current URL and propagates failures", async () => {
    usePreviewStore.getState().navigate("/ws/a", "https://example.com")
    const current = target()

    expect(await copyPreviewUrl(current)).toBe("completed")
    expect(commandMocks.writeText).toHaveBeenCalledWith("https://example.com")
    expect(await openPreviewExternally(current)).toBe("completed")
    expect(commandMocks.openUrl).toHaveBeenCalledWith("https://example.com")

    commandMocks.writeText.mockRejectedValueOnce(new Error("clipboard denied"))
    await expect(copyPreviewUrl(current)).rejects.toThrow("clipboard denied")
    commandMocks.openUrl.mockRejectedValueOnce(new Error("opener denied"))
    await expect(openPreviewExternally(current)).rejects.toThrow("opener denied")
  })

  it("cancels stale workspace and URL snapshots without falling back to the active preview", async () => {
    usePreviewStore.getState().navigate("/ws/a", "https://example.com/a")
    const staleUrl = target()
    usePreviewStore.getState().navigate("/ws/a", "https://example.com/b")

    expect(await reloadPreview(staleUrl)).toBe("cancelled")
    expect(await copyPreviewUrl(staleUrl)).toBe("cancelled")
    expect(await openPreviewExternally(staleUrl)).toBe("cancelled")

    const staleWorkspace = target()
    useWorkspaceStore.setState({ workspacePath: "/ws/b" })
    expect(await reloadPreview(staleWorkspace)).toBe("cancelled")
    expect(commandMocks.previewReload).not.toHaveBeenCalled()
    expect(commandMocks.writeText).not.toHaveBeenCalled()
    expect(commandMocks.openUrl).not.toHaveBeenCalled()
  })

  it("propagates native navigation failures", async () => {
    const preview = usePreviewStore.getState()
    preview.navigate("/ws/a", "https://example.com/first")
    preview.recordNativeOpen("/ws/a", "https://example.com/first")
    preview.navigate("/ws/a", "https://example.com/second")
    preview.recordNativeOpen("/ws/a", "https://example.com/second")
    const before = usePreviewStore.getState().navForWorkspace("/ws/a")
    commandMocks.previewBack.mockRejectedValueOnce(new Error("native back failed"))

    await expect(goBackPreview(target())).rejects.toThrow("native back failed")
    expect(usePreviewStore.getState().navForWorkspace("/ws/a")).toEqual(before)
    expect(usePreviewStore.getState().nativeNavigationSyncs["/ws/a"]).toBeUndefined()
  })

  it("does not synchronize native history when the URL snapshot becomes stale in flight", async () => {
    let resolveBack!: () => void
    commandMocks.previewBack.mockImplementationOnce(() => new Promise<void>((resolve) => {
      resolveBack = resolve
    }))
    const preview = usePreviewStore.getState()
    preview.navigate("/ws/a", "https://example.com/a")
    preview.recordNativeOpen("/ws/a", "https://example.com/a")
    preview.navigate("/ws/a", "https://example.com/b")
    preview.recordNativeOpen("/ws/a", "https://example.com/b")

    const navigating = goBackPreview(target())
    await vi.waitFor(() => expect(commandMocks.previewBack).toHaveBeenCalledTimes(1))
    preview.navigate("/ws/a", "https://example.com/c")
    resolveBack()

    await expect(navigating).resolves.toBe("cancelled")
    expect(usePreviewStore.getState().navForWorkspace("/ws/a").url)
      .toBe("https://example.com/c")
    expect(usePreviewStore.getState().nativeNavigationSyncs["/ws/a"]).toBeUndefined()
    expect(usePreviewStore.getState().nativeSession).toBeNull()
  })

  it("stops the exact running server and marks it exited only while its claim remains current", async () => {
    const preview = usePreviewStore.getState()
    const attempt = preview.beginAttempt("/ws/a")
    preview.setDevServer({
      workspace: "/ws/a",
      command: "bun run dev",
      port: 5173,
      status: { status: "running", port: 5173 },
    })
    commandMocks.devServerStop.mockResolvedValueOnce(undefined)
    const current = { ...target(), serverAttempt: attempt }

    expect(previewTargetHasRunningServer(current)).toBe(true)
    expect(await stopPreviewDevServer(current)).toBe("completed")
    expect(commandMocks.devServerStop).toHaveBeenCalledWith("/ws/a")
    expect(usePreviewStore.getState().devServerForWorkspace("/ws/a")?.status)
      .toEqual({ status: "exited", code: null })
  })

  it("restores the attempt and keeps the server running when stop fails", async () => {
    const preview = usePreviewStore.getState()
    const attempt = preview.beginAttempt("/ws/a")
    preview.setDevServer({
      workspace: "/ws/a",
      command: "bun run dev",
      port: 5173,
      status: { status: "running", port: 5173 },
    })
    commandMocks.devServerStop.mockRejectedValueOnce(new Error("stop failed"))
    const current = { ...target(), serverAttempt: attempt }

    await expect(stopPreviewDevServer(current)).rejects.toThrow("stop failed")
    expect(usePreviewStore.getState().attemptForWorkspace("/ws/a")).toBe(attempt)
    expect(usePreviewStore.getState().devServerForWorkspace("/ws/a")?.status.status)
      .toBe("running")
    expect(previewTargetHasRunningServer(current)).toBe(true)
  })

  it("does not overwrite a newer attempt after an in-flight stop resolves", async () => {
    let resolveStop!: () => void
    commandMocks.devServerStop.mockImplementationOnce(() => new Promise<void>((resolve) => {
      resolveStop = resolve
    }))
    const preview = usePreviewStore.getState()
    const attempt = preview.beginAttempt("/ws/a")
    preview.setDevServer({
      workspace: "/ws/a",
      command: "bun run dev",
      port: 5173,
      status: { status: "running", port: 5173 },
    })

    const stopping = stopPreviewDevServer({ ...target(), serverAttempt: attempt })
    usePreviewStore.getState().beginAttempt("/ws/a")
    resolveStop()
    await expect(stopping).resolves.toBe("completed")
    expect(usePreviewStore.getState().devServerForWorkspace("/ws/a")?.status.status)
      .toBe("running")
  })
})

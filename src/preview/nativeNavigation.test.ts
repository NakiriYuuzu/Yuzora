import { beforeEach, expect, it } from "vitest"
import { usePreviewStore } from "@/state/previewStore"
import { useWorkspaceStore } from "@/state/workspaceStore"

beforeEach(() => {
  useWorkspaceStore.setState({ workspacePath: "/ws/a" })
  usePreviewStore.getState().reset()
})

it("canonical initial load and redirect replace the requested URL without synthetic Back", () => {
  const s = usePreviewStore.getState()
  s.navigate("/ws/a", "http://localhost:34329")
  s.recordNativeOpen("/ws/a", "http://localhost:34329", "child-1")
  s.receiveNativeNavigation({ sessionId: "child-1", url: "http://localhost:34329/", canGoBack: false, canGoForward: false })
  expect(s.navForWorkspace("/ws/a")).toMatchObject({ url: "http://localhost:34329/", backStack: [] })
  s.receiveNativeNavigation({ sessionId: "child-1", url: "http://localhost:34329/final", canGoBack: false, canGoForward: false })
  expect(s.navForWorkspace("/ws/a")).toMatchObject({ url: "http://localhost:34329/final", backStack: [] })
})

it("uses native capabilities for link, in-page back, replaceState, hash and reload", () => {
  const s = usePreviewStore.getState()
  s.navigate("/ws/a", "https://example.com/")
  s.recordNativeOpen("/ws/a", "https://example.com/", "child-1")
  const snapshot = (url: string, canGoBack: boolean, canGoForward: boolean) =>
    s.receiveNativeNavigation({ sessionId: "child-1", url, canGoBack, canGoForward })
  snapshot("https://example.com/next", true, false)
  expect(usePreviewStore.getState().nativeSession).toMatchObject({ canGoBack: true, canGoForward: false })
  snapshot("https://example.com/", false, true)
  expect(usePreviewStore.getState().nativeSession).toMatchObject({ canGoBack: false, canGoForward: true })
  snapshot("https://example.com/replaced", false, true)
  snapshot("https://example.com/replaced#section", true, false)
  snapshot("https://example.com/replaced#section", true, false)
  expect(s.navForWorkspace("/ws/a")).toMatchObject({ url: "https://example.com/replaced#section", backStack: [] })
})

it("rejects stale child ownership, pending navigation and non-web URLs", () => {
  const s = usePreviewStore.getState()
  s.navigate("/ws/a", "https://example.com/")
  s.recordNativeOpen("/ws/a", "https://example.com/", "child-2")
  const snapshot = (sessionId: string, url: string) => s.receiveNativeNavigation({ sessionId, url, canGoBack: true, canGoForward: false })
  snapshot("child-1", "https://example.com/stale")
  snapshot("child-2", "file:///etc/passwd")
  s.beginNativeCloseRequest("/ws/a")
  snapshot("child-2", "https://example.com/late")
  expect(s.navForWorkspace("/ws/a").url).toBe("https://example.com/")
})

it("clears an outer Forward branch when the rebuilt native child gains a new history entry", () => {
  const s = usePreviewStore.getState()
  s.navigate("/ws/a", "https://example.com/a")
  s.navigate("/ws/a", "http://localhost:34329/static")
  s.goBack("/ws/a")
  s.recordNativeOpen("/ws/a", "https://example.com/a", "child-3")
  expect(usePreviewStore.getState().nativeSession?.outerForwardStack).toEqual(["http://localhost:34329/static"])
  s.receiveNativeNavigation({ sessionId: "child-3", url: "https://example.com/b", canGoBack: true, canGoForward: false })
  expect(usePreviewStore.getState().nativeSession?.outerForwardStack).toEqual([])
})

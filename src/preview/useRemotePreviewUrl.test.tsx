import { afterEach, beforeEach, expect, it, vi } from "vitest"
import { act, cleanup, renderHook, waitFor } from "@testing-library/react"
vi.mock("@/state/hostStore", () => ({ useHostStore: (selector: (state: unknown) => unknown) => selector({ hosts: {} }) }))
vi.mock("./remotePreviewUrl", async (original) => ({ ...await original<typeof import("./remotePreviewUrl")>(), acquireRemotePreviewUrl: vi.fn() }))
import { acquireRemotePreviewUrl } from "./remotePreviewUrl"
import { useRemotePreviewUrl } from "./useRemotePreviewUrl"
import { remoteFilePath } from "@/lib/runtimeIdentity"
beforeEach(() => { vi.clearAllMocks() })
afterEach(cleanup)

it("never renders the local machine's source port while a remote tunnel is opening or fails", async () => {
  vi.mocked(acquireRemotePreviewUrl).mockRejectedValue(new Error("offline"))
  const { result } = renderHook(() => useRemotePreviewUrl(remoteFilePath("offline", "/repo"), "http://localhost:5173", 0))
  expect(result.current.url).toBeNull()
  await waitFor(() => expect(result.current.error).toContain("offline"))
  expect(result.current.url).toBeNull()
})

it("releases a late lease after switching workspaces and keeps the new preview", async () => {
  let finish!: (value: { url: string; close: () => Promise<void> }) => void
  const closeOld = vi.fn(async () => {})
  const closeNew = vi.fn(async () => {})
  vi.mocked(acquireRemotePreviewUrl).mockImplementationOnce(() => new Promise((resolve) => { finish = resolve }))
    .mockResolvedValueOnce({ url: "http://127.0.0.1:43002/", close: closeNew })
  const { result, rerender, unmount } = renderHook(({ workspace }) => useRemotePreviewUrl(workspace, "http://localhost:5173/", 0), { initialProps: { workspace: remoteFilePath("old", "/repo") } })
  rerender({ workspace: remoteFilePath("new", "/repo") })
  await waitFor(() => expect(result.current.url).toBe("http://127.0.0.1:43002/"))
  await act(async () => finish({ url: "http://127.0.0.1:43001/", close: closeOld }))
  expect(closeOld).toHaveBeenCalledOnce()
  expect(result.current.url).toBe("http://127.0.0.1:43002/")
  unmount()
  expect(closeNew).toHaveBeenCalledOnce()
})

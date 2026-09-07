import { beforeEach, expect, it, vi } from "vitest"
vi.mock("@/lib/remoteFiles", () => ({ runtimeWorkspaceService: vi.fn() }))
vi.mock("@/lib/remoteTunnels", () => ({ openRemoteTunnel: vi.fn() }))
import { runtimeWorkspaceService } from "@/lib/remoteFiles"
import { openRemoteTunnel } from "@/lib/remoteTunnels"
import { remoteFilePath, parseRemoteFilePath } from "@/lib/runtimeIdentity"
import { acquireRemotePreviewUrl, needsRemotePreviewTunnel, remotePreviewDisplayUrl } from "./remotePreviewUrl"
const close = vi.fn(async () => {})
const assertCurrent = vi.fn()
beforeEach(() => {
  vi.clearAllMocks()
  assertCurrent.mockReset()
  vi.mocked(runtimeWorkspaceService).mockImplementation((uri) => ({ uri, root: "/repo", capabilityId: "cap", owner: { hostId: parseRemoteFilePath(uri)!.hostId, generation: 1 }, assertCurrent }))
  vi.mocked(openRemoteTunnel).mockResolvedValue({ port: 43001, close })
})

it("maps paths, queries and WebSocket origin to the same host tunnel without changing source navigation", async () => {
  const workspace = remoteFilePath("preview-a", "/repo")
  const source = "http://localhost:5173/about?q=中文#section"
  const lease = await acquireRemotePreviewUrl(workspace, source, () => {})
  expect(openRemoteTunnel).toHaveBeenCalledWith({ hostId: "preview-a", generation: 1 }, expect.stringContaining(workspace), { host: "localhost", port: 5173 }, expect.any(Function))
  const url = new URL(lease.url)
  expect(url.origin).toBe("http://127.0.0.1:43001")
  expect(url.pathname).toBe("/about")
  expect(url.searchParams.get("q")).toBe("中文")
  expect(url.hash).toBe("#section")
  expect(remotePreviewDisplayUrl(workspace, "http://localhost:5173/next")).toBe("http://127.0.0.1:43001/next")
  await lease.close()
  expect(() => remotePreviewDisplayUrl(workspace, source)).toThrow("not connected")
})

it("isolates identical ports across hosts and does not let a stale cleanup erase a new lease", async () => {
  const first = remoteFilePath("preview-first", "/repo")
  const second = remoteFilePath("preview-second", "/repo")
  const source = "http://localhost:5173/"
  const old = await acquireRemotePreviewUrl(first, source, () => {})
  vi.mocked(openRemoteTunnel).mockResolvedValue({ port: 43002, close })
  const other = await acquireRemotePreviewUrl(second, source, () => {})
  vi.mocked(openRemoteTunnel).mockResolvedValue({ port: 43003, close })
  const current = await acquireRemotePreviewUrl(first, source, () => {})
  await old.close()
  expect(remotePreviewDisplayUrl(first, source)).toBe("http://127.0.0.1:43003/")
  expect(remotePreviewDisplayUrl(second, source)).toBe("http://127.0.0.1:43002/")
  assertCurrent.mockImplementation(() => { throw new Error("host disconnected") })
  expect(() => remotePreviewDisplayUrl(first, source)).toThrow("disconnected")
  await current.close(); await other.close()
})

it("leaves native and external URLs unchanged", () => {
  expect(needsRemotePreviewTunnel("/native", "http://localhost:3000")).toBe(false)
  expect(needsRemotePreviewTunnel(remoteFilePath("host", "/repo"), "https://example.org")).toBe(false)
  expect(remotePreviewDisplayUrl("/native", "http://localhost:3000")).toBe("http://localhost:3000")
})

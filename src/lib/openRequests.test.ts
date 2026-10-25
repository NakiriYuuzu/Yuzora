import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"

import { showActionError } from "@/lib/actionFeedback"
import { canonicalRuntimeWorkspace } from "@/lib/herdrProvider"
import { takeOpenRequests } from "@/lib/ipc"
import { openExternalFiles } from "@/lib/openExternalFiles"
import {
  HERDR_SETTLE_TIMEOUT_MS,
  launchOpenRequests,
  openFolderFromOs,
  processOpenRequests,
  resetOpenRequestsForTest
} from "@/lib/openRequests"
import { isTauri } from "@/lib/platform"
import { openWorkspaceAtPath } from "@/lib/workspaceActions"

const herdr = vi.hoisted(() => ({
  state: {} as Record<string, unknown>,
  listeners: new Set<(state: Record<string, unknown>) => void>()
}))

vi.mock("@/state/herdrStore", () => ({
  pathsMatch: (a?: string | null, b?: string | null) => Boolean(a && b && a === b),
  useHerdrStore: {
    getState: () => herdr.state,
    subscribe: (listener: (state: Record<string, unknown>) => void) => {
      herdr.listeners.add(listener)
      return () => herdr.listeners.delete(listener)
    }
  }
}))
vi.mock("@/lib/herdrProvider", async (importOriginal) => ({
  ...(await importOriginal<typeof import("@/lib/herdrProvider")>()),
  canonicalRuntimeWorkspace: vi.fn()
}))
vi.mock("@/lib/ipc", () => ({ takeOpenRequests: vi.fn() }))
vi.mock("@/lib/platform", () => ({ isTauri: vi.fn() }))
vi.mock("@/lib/openExternalFiles", () => ({ openExternalFiles: vi.fn() }))
vi.mock("@/lib/workspaceActions", () => ({ openWorkspaceAtPath: vi.fn() }))
vi.mock("@/lib/actionFeedback", () => ({ showActionError: vi.fn() }))

const REMOTE_SESSION = JSON.stringify(["devbox", "default"])

function setHerdr(patch: Record<string, unknown>): void {
  herdr.state = { ...herdr.state, ...patch }
  for (const listener of [...herdr.listeners]) listener(herdr.state)
}

function readyLocalHerdr(overrides: Record<string, unknown> = {}): void {
  setHerdr({
    selectedSessionName: "default",
    connectionState: "ready",
    capabilities: {},
    herdrStartup: { state: "ready", error: null },
    spaces: () => [],
    canCreateSpace: () => true,
    activateSpace: vi.fn(async () => ({ ok: true })),
    createSpaceFromFolder: vi.fn(async () => ({ ok: true })),
    ...overrides
  })
}

beforeEach(() => {
  resetOpenRequestsForTest()
  herdr.state = {}
  herdr.listeners.clear()
  vi.mocked(isTauri).mockReset().mockReturnValue(true)
  vi.mocked(takeOpenRequests).mockReset().mockResolvedValue([])
  vi.mocked(openExternalFiles).mockReset().mockResolvedValue(undefined)
  vi.mocked(openWorkspaceAtPath).mockReset().mockResolvedValue(true)
  vi.mocked(showActionError).mockReset().mockResolvedValue(undefined)
  vi.mocked(canonicalRuntimeWorkspace).mockReset().mockImplementation(async (_scope, path) => `/canon${path}`)
})

afterEach(() => {
  vi.useRealTimers()
})

describe("launchOpenRequests", () => {
  it("takes the launch batch once and shares it", async () => {
    vi.mocked(takeOpenRequests).mockResolvedValue([{ path: "/a.md", kind: "file" }])

    const [first, second] = await Promise.all([launchOpenRequests(), launchOpenRequests()])

    expect(first).toEqual([{ path: "/a.md", kind: "file" }])
    expect(second).toBe(first)
    expect(takeOpenRequests).toHaveBeenCalledTimes(1)
  })

  it("is empty outside Tauri and when the native queue cannot be read", async () => {
    vi.mocked(isTauri).mockReturnValue(false)
    await expect(launchOpenRequests()).resolves.toEqual([])
    expect(takeOpenRequests).not.toHaveBeenCalled()

    resetOpenRequestsForTest()
    vi.mocked(isTauri).mockReturnValue(true)
    vi.mocked(takeOpenRequests).mockRejectedValue(new Error("ipc down"))
    await expect(launchOpenRequests()).resolves.toEqual([])
  })
})

describe("processOpenRequests", () => {
  it("opens in arrival order and batches consecutive files", async () => {
    setHerdr({ selectedSessionName: REMOTE_SESSION })
    const order: string[] = []
    vi.mocked(openExternalFiles).mockImplementation(async (paths) => { order.push(`files:${paths.join(",")}`) })
    vi.mocked(openWorkspaceAtPath).mockImplementation(async (path) => {
      order.push(`folder:${path}`)
      return true
    })

    await processOpenRequests([
      { path: "/a.md", kind: "file" },
      { path: "/b.md", kind: "file" },
      { path: "/project", kind: "directory" },
      { path: "/c.md", kind: "file" }
    ])

    expect(order).toEqual(["files:/a.md,/b.md", "folder:/project", "files:/c.md"])
    expect(openExternalFiles).toHaveBeenCalledWith(["/a.md", "/b.md"], "os")
  })

  it("runs later batches after earlier ones finish", async () => {
    let releaseFirst = () => {}
    vi.mocked(openExternalFiles)
      .mockImplementationOnce(() => new Promise<void>((resolve) => { releaseFirst = resolve }))
      .mockResolvedValue(undefined)

    const first = processOpenRequests([{ path: "/first.md", kind: "file" }])
    const second = processOpenRequests([{ path: "/second.md", kind: "file" }])
    await Promise.resolve()
    expect(openExternalFiles).toHaveBeenCalledTimes(1)

    releaseFirst()
    await Promise.all([first, second])
    expect(openExternalFiles).toHaveBeenLastCalledWith(["/second.md"], "os")
  })
})

describe("openFolderFromOs", () => {
  it("focuses the local Space already rooted at the folder", async () => {
    readyLocalHerdr({ spaces: () => [{ id: "space-1", path: "/canon/project" }] })

    await openFolderFromOs("/project")

    const state = herdr.state as { activateSpace: unknown; createSpaceFromFolder: unknown }
    expect(state.activateSpace).toHaveBeenCalledWith({
      sessionName: "default",
      workspaceId: "space-1",
      path: "/canon/project"
    })
    expect(state.createSpaceFromFolder).not.toHaveBeenCalled()
    expect(openWorkspaceAtPath).not.toHaveBeenCalled()
  })

  it("creates a Space named after the folder when none is rooted there", async () => {
    readyLocalHerdr()

    await openFolderFromOs("/project")

    expect((herdr.state as { createSpaceFromFolder: unknown }).createSpaceFromFolder)
      .toHaveBeenCalledWith("/canon/project", "project")
    expect(openWorkspaceAtPath).not.toHaveBeenCalled()
  })

  it("opens a plain workspace when the local Session cannot create a Space", async () => {
    readyLocalHerdr({ canCreateSpace: () => false })

    await openFolderFromOs("/project")

    expect(openWorkspaceAtPath).toHaveBeenCalledWith("/project")
  })

  it("leaves a remote Session alone and opens a plain workspace at once", async () => {
    setHerdr({ selectedSessionName: REMOTE_SESSION, connectionState: "connecting" })

    await openFolderFromOs("/project")

    expect(canonicalRuntimeWorkspace).not.toHaveBeenCalled()
    expect(openWorkspaceAtPath).toHaveBeenCalledWith("/project")
  })

  it("waits for HERDR to come up at launch before choosing", async () => {
    setHerdr({ selectedSessionName: null, connectionState: "idle", herdrStartup: { state: "starting", error: null } })

    const done = openFolderFromOs("/project")
    await Promise.resolve()
    expect(openWorkspaceAtPath).not.toHaveBeenCalled()
    readyLocalHerdr()
    await done

    expect((herdr.state as { createSpaceFromFolder: unknown }).createSpaceFromFolder)
      .toHaveBeenCalledWith("/canon/project", "project")
    expect(herdr.listeners.size).toBe(0)
  })

  it("falls back to a plain workspace when HERDR never settles", async () => {
    vi.useFakeTimers()
    setHerdr({ selectedSessionName: "default", connectionState: "connecting", herdrStartup: { state: "starting", error: null } })

    const done = openFolderFromOs("/project")
    await vi.advanceTimersByTimeAsync(HERDR_SETTLE_TIMEOUT_MS)
    await done

    expect(openWorkspaceAtPath).toHaveBeenCalledWith("/project")
    expect(herdr.listeners.size).toBe(0)
  })

  it("reports a failed activation but stays quiet when the user cancels", async () => {
    readyLocalHerdr({ createSpaceFromFolder: vi.fn(async () => ({ ok: false, error: "workspace.create failed" })) })
    await openFolderFromOs("/project")
    expect(showActionError).toHaveBeenCalledWith("Open folder", expect.objectContaining({ message: "workspace.create failed" }))

    vi.mocked(showActionError).mockClear()
    readyLocalHerdr({ createSpaceFromFolder: vi.fn(async () => ({ ok: false, cancelled: true })) })
    await openFolderFromOs("/project")
    expect(showActionError).not.toHaveBeenCalled()
  })

  it("reports a folder that cannot be opened as a workspace", async () => {
    setHerdr({ selectedSessionName: REMOTE_SESSION })
    vi.mocked(openWorkspaceAtPath).mockRejectedValue(new Error("invalid path"))

    await openFolderFromOs("/gone")

    expect(showActionError).toHaveBeenCalledWith("Open folder", expect.objectContaining({ message: "invalid path" }))
  })
})

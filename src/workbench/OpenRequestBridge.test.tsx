import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"
import { act, cleanup, render } from "@testing-library/react"

import { takeOpenRequests } from "@/lib/ipc"
import { launchOpenRequests, processOpenRequests } from "@/lib/openRequests"
import { useWorkspaceStore } from "@/state/workspaceStore"
import { OpenRequestBridge } from "@/workbench/OpenRequestBridge"

const eventMock = vi.hoisted(() => ({
  handler: null as (() => void) | null,
  unlisten: vi.fn(),
  registered: null as Promise<void> | null
}))

vi.mock("@tauri-apps/api/event", () => ({
  listen: (event: string, handler: () => void) => {
    if (event === "open-requests:available") eventMock.handler = handler
    return (eventMock.registered ?? Promise.resolve()).then(() => eventMock.unlisten)
  }
}))
vi.mock("@/lib/platform", () => ({ isTauri: () => true }))
vi.mock("@/lib/ipc", () => ({ takeOpenRequests: vi.fn() }))
vi.mock("@/lib/openRequests", () => ({
  launchOpenRequests: vi.fn(),
  processOpenRequests: vi.fn()
}))

async function flush(): Promise<void> {
  await act(async () => {
    for (let index = 0; index < 5; index += 1) await Promise.resolve()
  })
}

beforeEach(() => {
  eventMock.handler = null
  eventMock.unlisten.mockReset()
  eventMock.registered = null
  useWorkspaceStore.setState({ sessionRestoreReady: false })
  vi.mocked(launchOpenRequests).mockReset().mockResolvedValue([])
  vi.mocked(processOpenRequests).mockReset().mockResolvedValue(undefined)
  vi.mocked(takeOpenRequests).mockReset().mockResolvedValue([])
})

afterEach(() => {
  cleanup()
})

describe("OpenRequestBridge", () => {
  it("opens the launch batch only after the session restore settles", async () => {
    const launch = [{ path: "/notes.md", kind: "file" as const }]
    vi.mocked(launchOpenRequests).mockResolvedValue(launch)

    render(<OpenRequestBridge />)
    await flush()
    expect(processOpenRequests).not.toHaveBeenCalled()

    act(() => useWorkspaceStore.getState().markSessionRestoreReady())
    await flush()
    expect(processOpenRequests).toHaveBeenCalledWith(launch)
  })

  it("takes the native queue on each event, after the launch batch", async () => {
    const launch = [{ path: "/launch.md", kind: "file" as const }]
    const later = [{ path: "/project", kind: "directory" as const }]
    vi.mocked(launchOpenRequests).mockResolvedValue(launch)
    // The first take is the drain right after the listener registers.
    vi.mocked(takeOpenRequests).mockResolvedValueOnce([]).mockResolvedValueOnce(later)

    render(<OpenRequestBridge />)
    await flush()
    eventMock.handler?.()
    await flush()
    // A request arriving during the restore waits for it too.
    expect(takeOpenRequests).not.toHaveBeenCalled()

    act(() => useWorkspaceStore.getState().markSessionRestoreReady())
    await flush()
    expect(vi.mocked(processOpenRequests).mock.calls).toEqual([[launch], [later]])
  })

  it("takes a request that was queued before the listener registered", async () => {
    let register!: () => void
    eventMock.registered = new Promise((resolve) => { register = resolve })
    const missed = [{ path: "/missed.md", kind: "file" as const }]
    vi.mocked(takeOpenRequests).mockResolvedValueOnce(missed)
    act(() => useWorkspaceStore.getState().markSessionRestoreReady())

    render(<OpenRequestBridge />)
    await flush()
    // Its availability event fired while nothing listened.
    expect(processOpenRequests).not.toHaveBeenCalled()

    register()
    await flush()
    expect(vi.mocked(processOpenRequests).mock.calls).toEqual([[missed]])
  })

  it("stops opening requests once unmounted", async () => {
    vi.mocked(launchOpenRequests).mockResolvedValue([{ path: "/late.md", kind: "file" }])

    const { unmount } = render(<OpenRequestBridge />)
    await flush()
    unmount()
    act(() => useWorkspaceStore.getState().markSessionRestoreReady())
    await flush()

    expect(processOpenRequests).not.toHaveBeenCalled()
    expect(eventMock.unlisten).toHaveBeenCalled()
  })
})

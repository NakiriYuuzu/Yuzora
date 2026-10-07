import { act, cleanup, render } from "@testing-library/react"
import { afterEach, beforeEach, expect, it, vi } from "vitest"
import type { HerdrCapabilities, HerdrStartupStatus } from "@/lib/herdrTypes"
import { herdrInitialState, useHerdrStore } from "@/state/herdrStore"
import { HerdrBridge } from "./HerdrBridge"

const events = vi.hoisted(() => ({ listen: vi.fn(), unlisten: vi.fn() }))
vi.mock("@tauri-apps/api/event", () => ({ listen: events.listen }))
const original = useHerdrStore.getState()
let startupEvent: (event: { payload: HerdrStartupStatus }) => void

beforeEach(() => {
  vi.useFakeTimers()
  events.listen.mockReset().mockImplementation(async (name, callback) => {
    expect(name).toBe("herdr:startup")
    startupEvent = callback
    return events.unlisten
  })
  events.unlisten.mockReset()
  useHerdrStore.setState({
    ...original, ...herdrInitialState,
    refreshSessions: vi.fn(async () => {}),
    releaseAllAttachments: vi.fn(async () => {})
  }, true)
})
afterEach(() => {
  cleanup()
  useHerdrStore.setState(original, true)
  vi.restoreAllMocks()
  vi.useRealTimers()
})

it("polls every 4s visible, 30s hidden, and immediately on visibility recovery", async () => {
  const visibility = vi.spyOn(document, "visibilityState", "get").mockReturnValue("visible")
  const view = render(<HerdrBridge />)
  const poll = useHerdrStore.getState().refreshSessions
  await act(async () => {})
  expect(poll).toHaveBeenCalledOnce()
  await act(async () => vi.advanceTimersByTimeAsync(4000))
  expect(poll).toHaveBeenCalledTimes(2)
  visibility.mockReturnValue("hidden")
  act(() => document.dispatchEvent(new Event("visibilitychange")))
  await act(async () => vi.advanceTimersByTimeAsync(29_999))
  expect(poll).toHaveBeenCalledTimes(2)
  await act(async () => vi.advanceTimersByTimeAsync(1))
  expect(poll).toHaveBeenCalledTimes(3)
  visibility.mockReturnValue("visible")
  await act(async () => document.dispatchEvent(new Event("visibilitychange")))
  expect(poll).toHaveBeenCalledTimes(4)
  await act(async () => vi.advanceTimersByTimeAsync(4000))
  expect(poll).toHaveBeenCalledTimes(5)
  view.unmount()
  await act(async () => vi.advanceTimersByTimeAsync(60_000))
  act(() => document.dispatchEvent(new Event("visibilitychange")))
  expect(poll).toHaveBeenCalledTimes(5)
  expect(events.unlisten).toHaveBeenCalledOnce()
})

it("updates startup status and immediately polls even while hidden", async () => {
  vi.spyOn(document, "visibilityState", "get").mockReturnValue("hidden")
  render(<HerdrBridge />)
  await act(async () => {})
  const poll = useHerdrStore.getState().refreshSessions
  await act(async () => startupEvent({ payload: { state: "failed", error: "cannot start" } }))
  expect(useHerdrStore.getState().herdrStartup).toEqual({ state: "failed", error: "cannot start" })
  expect(poll).toHaveBeenCalledTimes(2)
})

it("queues startup discovery if a session listing is already in flight", async () => {
  let resolve!: () => void
  const poll = vi.fn().mockImplementationOnce(() => new Promise<void>(done => { resolve = done })).mockResolvedValue(undefined)
  useHerdrStore.setState({ refreshSessions: poll })
  render(<HerdrBridge />)
  await act(async () => startupEvent({ payload: { state: "ready", error: null } }))
  expect(poll).toHaveBeenCalledOnce()
  await act(async () => resolve())
  expect(poll).toHaveBeenCalledTimes(2)
})

it("runs queued discovery without repeating a completed tick snapshot and starts newly discovered identities", async () => {
  vi.spyOn(document, "visibilityState", "get").mockReturnValue("visible")
  const capabilities: HerdrCapabilities = {
    binarySource: { configured: "global", available: true, restartRequired: false },
    server: { running: true },
    api: {
      snapshot: true, ping: false, tabCreate: false, workspaceFocus: false,
      workspaceCreate: false, workspaceRename: false, workspaceClose: false,
      tabRename: false, tabClose: false, tabFocus: false, paneFocus: false,
      paneRename: false, paneSplit: false, paneZoom: false, paneSwap: false,
      paneClose: false, layoutExport: false, layoutSetSplitRatio: false,
      eventsSubscribe: false, worktreeList: false, methods: ["session.snapshot"]
    },
    terminal: { observe: false, control: false, takeover: false, input: false, resize: false, scroll: false, release: false, create: false },
    events: { status: "unavailable" }
  }
  const session = { name: "default", default: true, running: true, sessionDir: "/tmp/default", socketPath: "/tmp/default.sock" }
  let resolveDiscovery!: () => void
  const refreshSessions = vi.fn()
    .mockImplementationOnce(() => new Promise<void>(resolve => { resolveDiscovery = resolve }))
    .mockImplementation(async () => {
      useHerdrStore.setState({ sessions: [session, { ...session, name: "work", default: false, sessionDir: "/tmp/work", socketPath: "/tmp/work.sock" }] })
    })
  const refreshSnapshot = vi.fn(async () => true)
  const bootstrap = vi.fn(async () => {})
  useHerdrStore.setState({
    sessions: [session],
    runtimesBySession: { default: { connectionState: "ready", capabilities, snapshot: null, worktreeInventory: null, errorMessage: null } },
    refreshSessions, refreshSnapshot, bootstrap
  })
  render(<HerdrBridge />)
  await act(async () => {})
  expect(refreshSnapshot).toHaveBeenCalledExactlyOnceWith("default")
  // The tick finishes its snapshot while the first discovery is still pending.
  await act(async () => vi.advanceTimersByTimeAsync(4000))
  expect(refreshSnapshot).toHaveBeenCalledTimes(2)
  expect(refreshSessions).toHaveBeenCalledOnce()
  await act(async () => vi.advanceTimersByTimeAsync(100))
  await act(async () => resolveDiscovery())
  expect(refreshSessions).toHaveBeenCalledTimes(2)
  expect(bootstrap).toHaveBeenCalledExactlyOnceWith("work")
  expect(refreshSnapshot).toHaveBeenCalledTimes(2)
})

it("unlistens a late registration and ignores startup callbacks after unmount", async () => {
  let registered!: (unlisten: () => void) => void
  events.listen.mockImplementationOnce((_name, callback) => {
    startupEvent = callback
    return new Promise<() => void>(resolve => { registered = resolve })
  })
  const view = render(<HerdrBridge />)
  await act(async () => {})
  const poll = useHerdrStore.getState().refreshSessions
  view.unmount()
  await act(async () => registered(events.unlisten))
  await act(async () => startupEvent({ payload: { state: "failed", error: "stale" } }))
  expect(events.unlisten).toHaveBeenCalledOnce()
  expect(poll).toHaveBeenCalledOnce()
  expect(useHerdrStore.getState().herdrStartup.state).toBe("ready")
})

it("rebootstraps the local capability contract immediately after startup completion", async () => {
  const bootstrap = vi.fn(async () => {})
  useHerdrStore.setState({
    sessions: [{ name: "default", default: true, running: true, sessionDir: "/tmp/default", socketPath: "/tmp/default.sock" }],
    runtimesBySession: { default: { connectionState: "ready", capabilities: null, snapshot: null, worktreeInventory: null, errorMessage: null } },
    bootstrap
  })
  render(<HerdrBridge />)
  await act(async () => {})
  expect(bootstrap).not.toHaveBeenCalled()
  await act(async () => startupEvent({ payload: { state: "ready", error: null } }))
  expect(bootstrap).toHaveBeenCalledExactlyOnceWith("default")
})

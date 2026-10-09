import { act, cleanup, render } from "@testing-library/react"
import { afterEach, expect, it, vi } from "vitest"
import { useHostStore } from "@/state/hostStore"
import { useSshStore } from "@/state/sshStore"
import { useRuntimePreferencesStore } from "@/state/runtimePreferencesStore"
import { HostConnectionsBridge } from "./HostConnectionsBridge"

const original = useHostStore.getState()
const originalSsh = useSshStore.getState()
const originalPreferences = useRuntimePreferencesStore.getState()
afterEach(() => {
  cleanup()
  useHostStore.setState(original, true)
  useSshStore.setState(originalSsh, true)
  useRuntimePreferencesStore.setState(originalPreferences, true)
  vi.restoreAllMocks()
  vi.useRealTimers()
})

it("backs off hidden health checks without suppressing event-driven reconciliation", async () => {
  vi.useFakeTimers()
  const visibility = vi.spyOn(document, "visibilityState", "get").mockReturnValue("visible")
  const reconcile = vi.fn()
  useHostStore.setState({ reconcile })
  useRuntimePreferencesStore.setState({ hydrated: true })
  const view = render(<HostConnectionsBridge />)
  expect(reconcile).toHaveBeenCalledOnce()
  await act(async () => vi.advanceTimersByTimeAsync(4000))
  expect(reconcile).toHaveBeenCalledTimes(2)
  visibility.mockReturnValue("hidden")
  act(() => document.dispatchEvent(new Event("visibilitychange")))
  await act(async () => vi.advanceTimersByTimeAsync(29_999))
  expect(reconcile).toHaveBeenCalledTimes(2)
  await act(async () => vi.advanceTimersByTimeAsync(1))
  expect(reconcile).toHaveBeenCalledTimes(3)
  act(() => useSshStore.setState({ sessions: {
    fixture: { hostId: "fixture", sessionId: "ssh-fixture", status: "connected", fingerprint: null, knownHost: true, error: null },
  } }))
  act(() => useRuntimePreferencesStore.setState({ wslEnabled: !useRuntimePreferencesStore.getState().wslEnabled }))
  expect(reconcile).toHaveBeenCalledTimes(5)
  visibility.mockReturnValue("visible")
  act(() => document.dispatchEvent(new Event("visibilitychange")))
  expect(reconcile).toHaveBeenCalledTimes(6)
  await act(async () => vi.advanceTimersByTimeAsync(4000))
  expect(reconcile).toHaveBeenCalledTimes(7)
  view.unmount()
  await act(async () => vi.advanceTimersByTimeAsync(60_000))
  act(() => document.dispatchEvent(new Event("visibilitychange")))
  act(() => useSshStore.setState({ sessions: {} }))
  expect(reconcile).toHaveBeenCalledTimes(7)
})

it("waits for the WSL preference to hydrate before the first reconcile", async () => {
  const reconcile = vi.fn()
  let finish!: () => void
  const hydrate = vi.fn(() => new Promise<void>(done => { finish = () => { useRuntimePreferencesStore.setState({ hydrated: true }); done() } }))
  useHostStore.setState({ reconcile })
  useRuntimePreferencesStore.setState({ hydrated: false, hydrate })
  render(<HostConnectionsBridge />)
  expect(hydrate).toHaveBeenCalledOnce()
  expect(reconcile).not.toHaveBeenCalled()
  await act(async () => finish())
  expect(reconcile).toHaveBeenCalledOnce()
})

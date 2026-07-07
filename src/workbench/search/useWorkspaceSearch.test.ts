import { afterEach, describe, expect, it, vi } from "vitest"
import { act, renderHook } from "@testing-library/react"

import type { SearchEvent } from "@/lib/types"
import { useWorkspaceStore } from "@/state/workspaceStore"

const searchWorkspace = vi.fn(
  (_root: string, _query: string, _cs: boolean, _cb: (e: SearchEvent) => void) => Promise.resolve()
)

vi.mock("@/lib/ipc", () => ({
  searchWorkspace: (...args: Parameters<typeof searchWorkspace>) => searchWorkspace(...args),
}))

const { useWorkspaceSearch } = await import("@/workbench/search/useWorkspaceSearch")

afterEach(() => {
  vi.clearAllMocks()
  vi.useRealTimers()
  useWorkspaceStore.setState({ workspacePath: null })
})

describe("useWorkspaceSearch", () => {
  it("debounces the query by 250ms and forwards the trimmed query + case flag", async () => {
    vi.useFakeTimers()
    useWorkspaceStore.setState({ workspacePath: "/w" })

    const { rerender } = renderHook(
      ({ q, cs }: { q: string; cs: boolean }) => useWorkspaceSearch(q, cs),
      { initialProps: { q: "  foo  ", cs: false } }
    )

    // Nothing fires before the debounce window elapses.
    expect(searchWorkspace).not.toHaveBeenCalled()
    await act(async () => {
      await vi.advanceTimersByTimeAsync(250)
    })
    expect(searchWorkspace).toHaveBeenCalledWith("/w", "foo", false, expect.any(Function))

    // Flipping case-sensitivity re-runs the search with the new flag.
    searchWorkspace.mockClear()
    rerender({ q: "foo", cs: true })
    await act(async () => {
      await vi.advanceTimersByTimeAsync(250)
    })
    expect(searchWorkspace).toHaveBeenCalledWith("/w", "foo", true, expect.any(Function))
  })

  it("accumulates streamed events and clears loading on done", async () => {
    vi.useFakeTimers()
    useWorkspaceStore.setState({ workspacePath: "/w" })
    searchWorkspace.mockImplementation((_r, _q, _cs, cb) => {
      cb({ type: "match", path: "/w/a.ts", matches: [{ line: 1, col: 0, preview: "foo" }] })
      cb({ type: "done", truncated: false, fileCount: 1 })
      return Promise.resolve()
    })

    const { result } = renderHook(() => useWorkspaceSearch("foo", false))
    await act(async () => {
      await vi.advanceTimersByTimeAsync(250)
    })

    expect(result.current.events).toHaveLength(2)
    expect(result.current.loading).toBe(false)
  })

  it("drops stale responses from a superseded query", async () => {
    vi.useFakeTimers()
    useWorkspaceStore.setState({ workspacePath: "/w" })
    const callbacks: ((e: SearchEvent) => void)[] = []
    searchWorkspace.mockImplementation((_r, _q, _cs, cb) => {
      callbacks.push(cb)
      return Promise.resolve()
    })

    const { result, rerender } = renderHook(
      ({ q }: { q: string }) => useWorkspaceSearch(q, false),
      { initialProps: { q: "foo" } }
    )
    await act(async () => {
      await vi.advanceTimersByTimeAsync(250)
    })
    rerender({ q: "bar" })
    await act(async () => {
      await vi.advanceTimersByTimeAsync(250)
    })
    expect(callbacks).toHaveLength(2)

    // The first (superseded) callback must be ignored; only the live one counts.
    // A 50ms flush tick delivers the buffered live match to state.
    await act(async () => {
      callbacks[0]({ type: "match", path: "/w/stale.ts", matches: [{ line: 9, col: 0, preview: "x" }] })
      callbacks[1]({ type: "match", path: "/w/live.ts", matches: [{ line: 1, col: 0, preview: "y" }] })
      await vi.advanceTimersByTimeAsync(50)
    })
    expect(result.current.events).toEqual([
      { type: "match", path: "/w/live.ts", matches: [{ line: 1, col: 0, preview: "y" }] },
    ])
  })

  it("does not search below the 2-char minimum query length", async () => {
    vi.useFakeTimers()
    useWorkspaceStore.setState({ workspacePath: "/w" })
    renderHook(() => useWorkspaceSearch("a", false))
    await act(async () => {
      await vi.advanceTimersByTimeAsync(250)
    })
    expect(searchWorkspace).not.toHaveBeenCalled()
  })

  it("searches once the trimmed query reaches 2 chars", async () => {
    vi.useFakeTimers()
    useWorkspaceStore.setState({ workspacePath: "/w" })
    renderHook(() => useWorkspaceSearch(" ab ", false))
    await act(async () => {
      await vi.advanceTimersByTimeAsync(250)
    })
    expect(searchWorkspace).toHaveBeenCalledWith("/w", "ab", false, expect.any(Function))
  })

  it("buffers streamed matches and flushes them on a 50ms tick, not per event", async () => {
    vi.useFakeTimers()
    useWorkspaceStore.setState({ workspacePath: "/w" })
    let cb: (e: SearchEvent) => void = () => {}
    searchWorkspace.mockImplementation((_r, _q, _cs, c) => {
      cb = c
      return Promise.resolve()
    })

    const { result } = renderHook(() => useWorkspaceSearch("foo", false))
    await act(async () => {
      await vi.advanceTimersByTimeAsync(250)
    })

    // Two matches arrive with no `done`: they buffer instead of hitting state.
    act(() => {
      cb({ type: "match", path: "/w/a.ts", matches: [{ line: 1, col: 0, preview: "x" }] })
      cb({ type: "match", path: "/w/b.ts", matches: [{ line: 2, col: 0, preview: "y" }] })
    })
    expect(result.current.events).toHaveLength(0)

    // One flush tick delivers both buffered matches in a single setState.
    await act(async () => {
      await vi.advanceTimersByTimeAsync(50)
    })
    expect(result.current.events).toHaveLength(2)
  })

  it("clearing an active query fires an empty search to cancel the running one", async () => {
    vi.useFakeTimers()
    useWorkspaceStore.setState({ workspacePath: "/w" })

    const { rerender } = renderHook(
      ({ q }: { q: string }) => useWorkspaceSearch(q, false),
      { initialProps: { q: "foo" } }
    )
    await act(async () => {
      await vi.advanceTimersByTimeAsync(250)
    })
    searchWorkspace.mockClear()

    await act(async () => {
      rerender({ q: "" })
    })
    expect(searchWorkspace).toHaveBeenCalledWith("/w", "", false, expect.any(Function))
  })

  it("does not fire on first mount with an empty query", async () => {
    vi.useFakeTimers()
    useWorkspaceStore.setState({ workspacePath: "/w" })
    renderHook(() => useWorkspaceSearch("", false))
    await act(async () => {
      await vi.advanceTimersByTimeAsync(250)
    })
    expect(searchWorkspace).not.toHaveBeenCalled()
  })
})

import { act, cleanup, renderHook } from "@testing-library/react"
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"
import type { SearchEvent } from "@/lib/types"
import { useWorkspaceStore } from "@/state/workspaceStore"

const searchWorkspace = vi.fn<(
    root: string, query: string, caseSensitive: boolean, onEvent: (event: SearchEvent) => void
) => Promise<void>>()
vi.mock("@/lib/ipc", () => ({
    searchWorkspace: (...args: Parameters<typeof searchWorkspace>) => searchWorkspace(...args),
}))
const { useWorkspaceSearch } = await import("./useWorkspaceSearch")

const match = (name: string): SearchEvent => ({
    type: "match", path: `/fixture/${name}.txt`, matches: [{ line: 1, col: 0, preview: name }],
})
const done: SearchEvent = { type: "done", truncated: false, fileCount: 1 }
const requests: {
    query: string
    onEvent: (event: SearchEvent) => void
    resolve: () => void
    reject: (error: unknown) => void
}[] = []

beforeEach(() => {
    vi.useFakeTimers()
    searchWorkspace.mockReset()
    requests.length = 0
    useWorkspaceStore.setState({ workspacePath: "/fixture" })
    searchWorkspace.mockImplementation((_root, query, _caseSensitive, onEvent) => new Promise<void>((resolve, reject) => {
        requests.push({ query, onEvent, resolve, reject })
    }))
})
afterEach(() => {
    cleanup()
    vi.restoreAllMocks()
    vi.useRealTimers()
    useWorkspaceStore.setState({ workspacePath: null })
})
const advance = async (ms: number) => {
    await act(async () => { await vi.advanceTimersByTimeAsync(ms) })
}
async function start() {
    const view = renderHook(({ query }) => useWorkspaceSearch(query, false), { initialProps: { query: "needle" } })
    await advance(250)
    expect(requests).toHaveLength(1)
    expect(view.result.current.loading).toBe(true)
    expect(vi.getTimerCount()).toBe(1)
    return view
}

describe("workspace search invocation lifecycle", () => {
    it.each([new Error("channel unavailable"), "connection closed"])("settles a current invocation failure and releases its timer: %s", async error => {
        const { result } = await start()
        await act(async () => { requests[0].reject(error) })
        expect(result.current.loading).toBe(false)
        expect(vi.getTimerCount()).toBe(0)
        expect(result.current.error).toBe(error instanceof Error ? error.message : error)
        expect(result.current.events).toEqual([])
        await advance(1000)
        expect(vi.getTimerCount()).toBe(0)
    })

    it("preserves published and buffered matches exactly once, then rejects late events", async () => {
        const { result } = await start()
        act(() => { requests[0].onEvent(match("published")) })
        await advance(50)
        expect(result.current.events).toEqual([match("published")])
        act(() => { requests[0].onEvent(match("buffered")) })
        await act(async () => { requests[0].reject(new Error("search interrupted")) })
        expect(result.current.events).toEqual([match("published"), match("buffered")])
        expect(result.current.loading).toBe(false)
        expect(result.current.error).toBe("search interrupted")
        expect(vi.getTimerCount()).toBe(0)
        act(() => { requests[0].onEvent(match("late")); requests[0].onEvent(done) })
        await advance(100)
        expect(result.current.events).toEqual([match("published"), match("buffered")])
        expect(result.current.error).toBe("search interrupted")
    })

    it("keeps streaming after the start acknowledgement resolves", async () => {
        const { result } = await start()
        await act(async () => { requests[0].resolve() })
        expect(result.current.loading).toBe(true)
        expect(vi.getTimerCount()).toBe(1)
        act(() => { requests[0].onEvent(match("valid")); requests[0].onEvent(done) })
        expect(result.current.events).toEqual([match("valid"), done])
        expect(result.current.loading).toBe(false)
        expect(vi.getTimerCount()).toBe(0)
    })

    it("does not overwrite completed results when the acknowledgement later rejects", async () => {
        const { result } = await start()
        act(() => { requests[0].onEvent(match("valid")); requests[0].onEvent(done) })
        await act(async () => { requests[0].reject(new Error("late acknowledgement")) })
        expect(result.current.events).toEqual([match("valid"), done])
        expect(result.current.loading).toBe(false)
        expect(result.current.error).toBeNull()
        expect(vi.getTimerCount()).toBe(0)
    })

    it.each(["query", "workspace", "workspace ABA"])("ignores retired rejection across %s without clearing the new timer", async change => {
        const view = await start()
        act(() => { requests[0].onEvent(match("retired")) })
        if (change === "query") view.rerender({ query: "second" })
        else {
            act(() => { useWorkspaceStore.setState({ workspacePath: "/other" }) })
            if (change === "workspace ABA") act(() => { useWorkspaceStore.setState({ workspacePath: "/fixture" }) })
        }
        await advance(250)
        expect(requests).toHaveLength(2)
        await act(async () => { requests[0].reject(new Error("stale")) })
        expect(view.result.current.loading).toBe(true)
        expect(view.result.current.error).toBeNull()
        expect(view.result.current.events).toEqual([])
        expect(vi.getTimerCount()).toBe(1)
        act(() => { requests[1].onEvent(match("current")) })
        await advance(50)
        expect(view.result.current.events).toEqual([match("current")])
        act(() => { requests[1].onEvent(done) })
        expect(vi.getTimerCount()).toBe(0)
    })

    it("logs clear-query cancellation failure without reviving search or poisoning its successor", async () => {
        const warn = vi.spyOn(console, "warn").mockImplementation(() => {})
        const view = await start()
        view.rerender({ query: "" })
        expect(requests).toHaveLength(2)
        expect(requests[1].query).toBe("")
        expect(vi.getTimerCount()).toBe(0)
        view.rerender({ query: "second" })
        await advance(250)
        await act(async () => { requests[1].reject(new Error("cancel unavailable")) })
        expect(warn).toHaveBeenCalledWith("Workspace search cancellation failed", expect.any(Error))
        expect(view.result.current.loading).toBe(true)
        expect(view.result.current.error).toBeNull()
        expect(view.result.current.events).toEqual([])
        expect(vi.getTimerCount()).toBe(1)
        act(() => { requests[2].onEvent(done) })
        expect(vi.getTimerCount()).toBe(0)
    })

    it("clears errors for the next query and when the root is removed", async () => {
        const view = await start()
        await act(async () => { requests[0].reject(new Error("first failure")) })
        expect(view.result.current.error).toBe("first failure")
        view.rerender({ query: "second" })
        expect(view.result.current.error).toBeNull()
        await advance(250)
        await act(async () => { requests[1].reject(new Error("second failure")) })
        expect(view.result.current.error).toBe("second failure")
        act(() => { useWorkspaceStore.setState({ workspacePath: null }) })
        expect(view.result.current).toEqual({ events: [], loading: false, error: null })
        expect(vi.getTimerCount()).toBe(0)
    })

    it("settles a rejected promise after unmount without leaving a timer", async () => {
        const view = await start()
        view.unmount()
        expect(vi.getTimerCount()).toBe(0)
        await act(async () => { requests[0].reject(new Error("unmounted")) })
        await advance(100)
        expect(vi.getTimerCount()).toBe(0)
    })
})

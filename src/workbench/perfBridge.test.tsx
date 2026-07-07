import { afterEach, beforeEach, expect, it, vi } from "vitest"
import { cleanup, render } from "@testing-library/react"

const perfSnapshot = vi.fn()
vi.mock("../lib/ipc", () => ({
    perfSnapshot: () => perfSnapshot()
}))

import { PerfBridge } from "./PerfBridge"
import { usePerfStore } from "../state/perfStore"

beforeEach(() => {
    vi.useFakeTimers()
    perfSnapshot.mockReset()
    perfSnapshot.mockResolvedValue({ cpuPercent: 12, memoryBytes: 184_000_000 })
    usePerfStore.getState().reset()
    vi.spyOn(document, "hasFocus").mockReturnValue(true)
})

afterEach(() => {
    cleanup()
    vi.runOnlyPendingTimers()
    vi.useRealTimers()
    vi.restoreAllMocks()
})

it("polls perf_snapshot every 2000ms and feeds the store", async () => {
    render(<PerfBridge />)
    // No immediate poll — the first sample lands one interval in.
    expect(perfSnapshot).not.toHaveBeenCalled()

    await vi.advanceTimersByTimeAsync(2000)
    expect(perfSnapshot).toHaveBeenCalledTimes(1)
    expect(usePerfStore.getState().snapshot).toEqual({
        cpuPercent: 12,
        memoryBytes: 184_000_000
    })

    await vi.advanceTimersByTimeAsync(2000)
    expect(perfSnapshot).toHaveBeenCalledTimes(2)
})

it("skips the poll while the window is unfocused", async () => {
    vi.mocked(document.hasFocus).mockReturnValue(false)
    render(<PerfBridge />)

    await vi.advanceTimersByTimeAsync(2000)
    expect(perfSnapshot).not.toHaveBeenCalled()
})

it("clears the interval on unmount", async () => {
    const { unmount } = render(<PerfBridge />)
    await vi.advanceTimersByTimeAsync(2000)
    expect(perfSnapshot).toHaveBeenCalledTimes(1)

    unmount()
    await vi.advanceTimersByTimeAsync(4000)
    expect(perfSnapshot).toHaveBeenCalledTimes(1)
})

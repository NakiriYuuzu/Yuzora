import { afterEach, expect, it, vi } from "vitest"
import { createPaneScrollController, type PaneScrollInfo } from "./herdrScrollController"

const base = { offsetFromBottom: 0, maxOffsetFromBottom: 100, viewportRows: 24 }
afterEach(() => vi.useRealTimers())

it("retains a trailing frame refresh when output arrives during an unresolved read", async () => {
  vi.useFakeTimers()
  let resolve!: (state: PaneScrollInfo) => void
  const read = vi.fn().mockImplementationOnce(() => new Promise(done => { resolve = done })).mockResolvedValue(base)
  const controller = createPaneScrollController({ read, write: vi.fn(), allowed: () => true, change: vi.fn() })
  const initial = controller.refresh()
  controller.frame()
  await vi.advanceTimersByTimeAsync(500)
  expect(read).toHaveBeenCalledOnce()
  resolve(base)
  await initial
  await vi.advanceTimersByTimeAsync(1)
  expect(read).toHaveBeenCalledTimes(2)
  await vi.advanceTimersByTimeAsync(1000)
  expect(read).toHaveBeenCalledTimes(2)
  controller.dispose()
})

it.each(["reset", "dispose"] as const)("cancels scheduled frame work on %s", async method => {
  vi.useFakeTimers()
  const read = vi.fn().mockResolvedValue(base)
  const controller = createPaneScrollController({ read, write: vi.fn(), allowed: () => true, change: vi.fn() })
  await controller.refresh()
  controller.frame()
  controller[method]()
  await vi.advanceTimersByTimeAsync(1000)
  expect(read).toHaveBeenCalledOnce()
  controller.dispose()
})

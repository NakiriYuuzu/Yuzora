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

it("follows connector-wheel frames at gesture cadence, then returns to the output throttle", async () => {
  vi.useFakeTimers()
  const read = vi.fn().mockResolvedValue(base)
  const controller = createPaneScrollController({ read, write: vi.fn(), allowed: () => true, change: vi.fn() })
  await controller.refresh()
  // An output frame just before the wheel must not hold the gesture at 350ms.
  controller.frame()
  controller.follow()
  for (let i = 0; i < 10; i++) {
    controller.frame()
    await vi.advanceTimersByTimeAsync(40)
  }
  expect(read.mock.calls.length).toBeGreaterThanOrEqual(10)
  read.mockClear()
  await vi.advanceTimersByTimeAsync(1000)
  for (let i = 0; i < 10; i++) {
    controller.frame()
    await vi.advanceTimersByTimeAsync(100)
  }
  expect(read).toHaveBeenCalledTimes(3)
  controller.dispose()
})

it("does not follow while the scrollbar is not allowed", async () => {
  vi.useFakeTimers()
  let allowed = true
  const read = vi.fn().mockResolvedValue(base)
  const controller = createPaneScrollController({ read, write: vi.fn(), allowed: () => allowed, change: vi.fn() })
  await controller.refresh()
  allowed = false
  controller.follow()
  allowed = true
  read.mockClear()
  for (let i = 0; i < 10; i++) {
    controller.frame()
    await vi.advanceTimersByTimeAsync(40)
  }
  expect(read).toHaveBeenCalledTimes(1)
  controller.dispose()
})

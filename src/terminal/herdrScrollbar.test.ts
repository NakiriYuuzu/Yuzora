import { expect, it, vi } from "vitest"
import { createPaneScrollController, scrollProxyContentHeight, offsetFromProxyScroll, proxyScrollTop } from "./herdrScrollController"
const state = { offsetFromBottom: 0, maxOffsetFromBottom: 100, viewportRows: 25 }
it("maps the shadcn proxy to server rows, with top=maximum and bottom=zero", () => {
  expect(scrollProxyContentHeight(state, 200)).toBe(1000)
  expect(proxyScrollTop(state, 800)).toBe(800)
  expect(offsetFromProxyScroll(state, 0, 800)).toBe(100)
  expect(offsetFromProxyScroll(state, 800, 800)).toBe(0)
  expect(offsetFromProxyScroll(state, 400, 800)).toBe(50)
  expect(scrollProxyContentHeight({ ...state, maxOffsetFromBottom: 0 }, 200)).toBe(200)
})
it("coalesces dragging to the latest absolute position and drops late closed-pane replies", async () => {
  let resolve!: (value: typeof state) => void
  const write = vi.fn().mockImplementationOnce(() => new Promise((done) => { resolve = done })).mockResolvedValue(state)
  const change = vi.fn()
  const controller = createPaneScrollController({ read: async () => state, write, allowed: () => true, change })
  await controller.refresh()
  controller.move(20); controller.move(40); controller.move(80)
  expect(write).toHaveBeenCalledTimes(1)
  resolve({ ...state, offsetFromBottom: 20 })
  await vi.waitFor(() => expect(write).toHaveBeenCalledTimes(2))
  expect(write.mock.calls.map(([offset]) => offset)).toEqual([20, 80])
  controller.dispose()
  const count = change.mock.calls.length
  await Promise.resolve()
  controller.move(0)
  expect(change).toHaveBeenCalledTimes(count)
})
it("does not send mutations for readonly or alternate-screen terminals or invent a missing range", async () => {
  const write = vi.fn()
  let allowed = false
  const controller = createPaneScrollController({ read: async () => state, write, allowed: () => allowed, change: vi.fn() })
  await controller.refresh(); controller.move(20)
  expect(write).not.toHaveBeenCalled()
  allowed = true
  const missing = createPaneScrollController({ read: async () => null, write, allowed: () => true, change: vi.fn() })
  await missing.refresh(); missing.move(20)
  expect(write).not.toHaveBeenCalled()
  controller.dispose(); missing.dispose()
})

it("discards a read from a closed pane and a pre-drag read that arrives after an absolute move", async () => {
  let resolveRead!: (value: typeof state) => void
  const change = vi.fn()
  const closed = createPaneScrollController({ read: () => new Promise((done) => { resolveRead = done }), write: vi.fn(), allowed: () => true, change })
  const read = closed.refresh()
  closed.dispose()
  resolveRead(state)
  await read
  expect(change).not.toHaveBeenCalled()
  const updates = vi.fn()
  const source = vi.fn().mockResolvedValueOnce(state).mockImplementationOnce(() => new Promise((done) => { resolveRead = done }))
  const controller = createPaneScrollController({ read: source, write: async (offset) => ({ ...state, offsetFromBottom: offset }), allowed: () => true, change: updates })
  await controller.refresh()
  const oldRead = controller.refresh()
  controller.move(75)
  await Promise.resolve()
  resolveRead(state)
  await oldRead
  expect(updates).toHaveBeenLastCalledWith({ ...state, offsetFromBottom: 75 })
  controller.dispose()
})

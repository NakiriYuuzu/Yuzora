import { beforeEach, expect, it, vi } from "vitest"
import { createHerdrTerminalTransport } from "./terminalTransport"
import { createPaneScrollController } from "./herdrScrollController"
import { herdrTerminalOpen, herdrTerminalScroll } from "@/lib/herdrIpc"
import { readPaneScroll, setPaneScroll } from "./herdrScrollIpc"

vi.mock("@/lib/herdrIpc", () => ({ herdrTerminalOpen: vi.fn(), herdrTerminalScroll: vi.fn(), herdrTerminalRelease: vi.fn() }))
vi.mock("./herdrScrollIpc", () => ({ readPaneScroll: vi.fn(), setPaneScroll: vi.fn() }))
const base = { offsetFromBottom: 0, maxOffsetFromBottom: 2000, viewportRows: 24 }
function deferred<T>() {
  let resolve!: (value: T) => void
  const promise = new Promise<T>((done) => { resolve = done })
  return { promise, resolve }
}
beforeEach(() => {
  vi.resetAllMocks()
  vi.mocked(readPaneScroll).mockResolvedValue(base)
  vi.mocked(setPaneScroll).mockReturnValue(new Promise(() => {}))
  vi.mocked(herdrTerminalOpen).mockResolvedValue({ sessionId: "connector", target: "t1", mode: "control", role: "controller", cols: 80, rows: 24, takeover: true })
})

it("coalesces wheel events during initial hydration without polling or inventing history", async () => {
  const hydration = deferred<typeof base>()
  const read = vi.fn().mockReturnValue(hydration.promise)
  const write = vi.fn().mockImplementation(async (offset) => ({ ...base, offsetFromBottom: offset }))
  const change = vi.fn()
  const controller = createPaneScrollController({ read, write, change, allowed: () => true })
  controller.scroll(-3); controller.scroll(-5); controller.scroll(2)
  expect(read).toHaveBeenCalledOnce()
  expect(write).not.toHaveBeenCalled()
  expect(change).not.toHaveBeenCalled()
  hydration.resolve(base)
  await vi.waitFor(() => expect(write.mock.calls.map(([offset]) => offset)).toEqual([6]))
  controller.dispose()
})

it("ignores a late acknowledgement after reconnect and hydrates the new generation", async () => {
  const old = deferred<typeof base>()
  const change = vi.fn()
  const controller = createPaneScrollController({ read: async () => base, write: () => old.promise, change, allowed: () => true })
  await controller.refresh()
  controller.move(100)
  controller.reset()
  change.mockClear()
  old.resolve({ ...base, offsetFromBottom: 100 })
  await vi.waitFor(() => expect(change).toHaveBeenLastCalledWith(base))
  expect(change).not.toHaveBeenCalledWith({ ...base, offsetFromBottom: 100 })
  controller.dispose()
})

it("reports background failure and drops unsent scrolling without replaying it", async () => {
  let reject!: (reason: Error) => void
  const write = vi.fn().mockReturnValue(new Promise((_, fail) => { reject = fail }))
  const error = vi.fn()
  const controller = createPaneScrollController({ read: async () => base, write, error, change: vi.fn(), allowed: () => true })
  await controller.refresh()
  controller.scroll(-2); controller.scroll(-8)
  reject(new Error("permission denied"))
  await vi.waitFor(() => expect(error).toHaveBeenCalled())
  await controller.refresh()
  expect(write).toHaveBeenCalledTimes(1)
  controller.dispose()
})

it("dispatches WSL wheel immediately, shares drag position, then reconciles overflow in the background", async () => {
  const first = deferred<typeof base>()
  const second = deferred<typeof base>()
  const change = vi.fn()
  const write = vi.fn()
    .mockReturnValueOnce(first.promise)
    .mockReturnValueOnce(second.promise)
  const controller = createPaneScrollController({ read: async () => base, write, change, allowed: () => true })
  await controller.refresh()
  const transport = createHerdrTerminalTransport({ terminalId: "t1", paneId: "p1", sessionName: "[wsl:Ubuntu-26.04,default]", paneScrollEnabled: () => true, terminalScrollEnabled: () => false, paneScrollController: () => controller })
  await transport.open({ cols: 80, rows: 24, onEvent: vi.fn() })
  let dispatched = false
  void transport.scroll!(-3).then(() => { dispatched = true })
  await Promise.resolve()
  await Promise.resolve()
  expect(dispatched).toBe(true)
  expect(change).toHaveBeenLastCalledWith({ ...base, offsetFromBottom: 3 })
  controller.move(800)
  await transport.scroll!(-5)
  expect(change).toHaveBeenLastCalledWith({ ...base, offsetFromBottom: 805 })
  expect(write.mock.calls.map(([offset]) => offset)).toEqual([3])
  first.resolve({ ...base, offsetFromBottom: 3, maxOffsetFromBottom: 2100 })
  await vi.waitFor(() => expect(write.mock.calls.map(([offset]) => offset)).toEqual([3, 805]))
  await vi.waitFor(() => expect(change).toHaveBeenLastCalledWith({ ...base, offsetFromBottom: 805, maxOffsetFromBottom: 2100 }))
  // Old acknowledgement can update the overflow, but never move the latest thumb backwards.
  expect(change).toHaveBeenLastCalledWith({ ...base, offsetFromBottom: 805, maxOffsetFromBottom: 2100 })
  second.resolve({ ...base, offsetFromBottom: 805, maxOffsetFromBottom: 2200 })
  await vi.waitFor(() => expect(change).toHaveBeenLastCalledWith({ ...base, offsetFromBottom: 805, maxOffsetFromBottom: 2200 }))
  expect(readPaneScroll).not.toHaveBeenCalled()
  expect(setPaneScroll).not.toHaveBeenCalled()
  expect(herdrTerminalScroll).not.toHaveBeenCalled()
  controller.dispose()
})

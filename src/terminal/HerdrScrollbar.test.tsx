import { act, cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react"
import { afterEach, beforeEach, expect, it, vi } from "vitest"
import { HerdrScrollbar } from "./HerdrScrollbar"
import { readPaneScroll, setPaneScroll } from "./herdrScrollIpc"
vi.mock("./herdrScrollIpc", () => ({ readPaneScroll: vi.fn(), setPaneScroll: vi.fn() }))
let resizeCallbacks: Array<() => void> = []
const base = { offsetFromBottom: 0, maxOffsetFromBottom: 100, viewportRows: 25 }
beforeEach(() => {
  vi.mocked(readPaneScroll).mockReset().mockResolvedValue(base)
  vi.mocked(setPaneScroll).mockReset().mockImplementation(async (_session, _pane, offset) => ({ ...base, offsetFromBottom: offset }))
  resizeCallbacks = []
  vi.stubGlobal("ResizeObserver", class {
    constructor(callback: () => void) { resizeCallbacks.push(callback) }
    observe() {} unobserve() {} disconnect() {}
  })
  vi.spyOn(HTMLElement.prototype, "clientHeight", "get").mockReturnValue(200)
  vi.spyOn(HTMLElement.prototype, "offsetHeight", "get").mockReturnValue(200)
  vi.spyOn(HTMLElement.prototype, "scrollHeight", "get").mockImplementation(function (this: HTMLElement) {
    return Number.parseFloat((this.querySelector("[data-herdr-scroll-extent]") as HTMLElement | null)?.style.height ?? "200") || 200
  })
})
afterEach(() => { cleanup(); vi.restoreAllMocks(); vi.unstubAllGlobals(); vi.useRealTimers() })
it("uses the real shadcn track and thumb to scroll the server without moving input focus", async () => {
  const ancestor = vi.fn()
  const view = render(<div onPointerDown={ancestor}><input /><HerdrScrollbar sessionName="work" paneId="w1:p1" enabled canScroll={() => true} refreshRef={{ current: null }} viewportId="terminal" /></div>)
  const scrollbar = await screen.findByRole("scrollbar")
  const proxy = screen.getByTestId("herdr-scroll-proxy")
  expect(proxy.scrollHeight).toBe(1000)
  expect(proxy.scrollTop).toBe(800)
  fireEvent.scroll(proxy)
  expect(setPaneScroll).not.toHaveBeenCalled()
  act(() => resizeCallbacks.forEach((callback) => callback()))
  const bar = await waitFor(() => {
    const bar = scrollbar.querySelector('[data-slot="scroll-area-scrollbar"]') as HTMLElement
    expect(bar).toBeTruthy()
    return bar
  })
  act(() => resizeCallbacks.forEach((callback) => callback()))
  const thumb = await waitFor(() => {
    const thumb = bar.querySelector('[data-slot="scroll-area-thumb"]') as HTMLElement
    expect(thumb).toBeTruthy()
    return thumb
  })
  vi.spyOn(bar, "getBoundingClientRect").mockReturnValue({ top: 0, left: 0, width: 10, height: 200, bottom: 200, right: 10 } as DOMRect)
  vi.spyOn(thumb, "getBoundingClientRect").mockImplementation(() => ({ top: proxy.scrollTop / 5, left: 0, width: 10, height: 40 } as DOMRect))
  const input = view.container.querySelector("input")!
  input.focus()
  fireEvent.pointerDown(bar, { button: 0, pointerId: 1, clientY: 0 })
  fireEvent.scroll(proxy)
  await waitFor(() => expect(setPaneScroll).toHaveBeenLastCalledWith("work", "w1:p1", 100))
  fireEvent.pointerUp(bar, { pointerId: 1 })
  fireEvent.pointerDown(thumb, { button: 0, pointerId: 2, clientY: 10 })
  fireEvent.pointerMove(bar, { pointerId: 2, clientY: 170 })
  fireEvent.scroll(proxy)
  fireEvent.pointerUp(bar, { pointerId: 2 })
  await waitFor(() => expect(setPaneScroll).toHaveBeenLastCalledWith("work", "w1:p1", 0))
  expect(document.activeElement).toBe(input)
  expect(ancestor).not.toHaveBeenCalled()
})
it("does not poll hidden, readonly or alternate-screen panes and rechecks control before dragging", async () => {
  let allowed = true
  const props = { sessionName: "work", paneId: "w1:p1", enabled: true, canScroll: () => allowed, refreshRef: { current: null }, viewportId: "terminal" }
  const view = render(<HerdrScrollbar {...props} />)
  const bar = await screen.findByRole("scrollbar")
  allowed = false
  fireEvent.pointerDown(bar, { button: 0, pointerId: 1, clientY: 0 })
  expect(setPaneScroll).not.toHaveBeenCalled()
  view.rerender(<HerdrScrollbar {...props} enabled={false} />)
  vi.mocked(readPaneScroll).mockClear()
  vi.useFakeTimers()
  act(() => vi.advanceTimersByTime(5000))
  expect(readPaneScroll).not.toHaveBeenCalled()
  view.rerender(<HerdrScrollbar {...props} />)
  act(() => vi.advanceTimersByTime(5000))
  expect(readPaneScroll).not.toHaveBeenCalled()
})
it("does not invent a thumb when HERDR omits its range", async () => {
  vi.mocked(readPaneScroll).mockResolvedValue(null)
  const view = render(<HerdrScrollbar sessionName="work" paneId="w1:p1" enabled canScroll={() => true} refreshRef={{ current: null }} viewportId="terminal" />)
  await waitFor(() => expect(readPaneScroll).toHaveBeenCalledOnce())
  expect(view.container.querySelector('[data-slot="scroll-area-thumb"]')).toBeNull()
  expect(screen.queryByRole("scrollbar")).toBeNull()
})

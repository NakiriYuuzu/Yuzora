import { act, cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react"
import { afterEach, beforeEach, expect, it, vi } from "vitest"
import { HerdrScrollbar } from "./HerdrScrollbar"
import { readPaneScroll, setPaneScroll } from "./herdrScrollIpc"
import type { PaneScrollController } from "./herdrScrollController"
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
  await waitFor(() => expect(setPaneScroll).toHaveBeenLastCalledWith("work", "w1:p1", 100, expect.any(AbortSignal)))
  fireEvent.pointerUp(bar, { pointerId: 1 })
  fireEvent.pointerDown(thumb, { button: 0, pointerId: 2, clientY: 10 })
  fireEvent.pointerMove(bar, { pointerId: 2, clientY: 170 })
  fireEvent.scroll(proxy)
  fireEvent.pointerUp(bar, { pointerId: 2 })
  await waitFor(() => expect(setPaneScroll).toHaveBeenLastCalledWith("work", "w1:p1", 0, expect.any(AbortSignal)))
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

it("keeps wheel and drag responsive while delayed acknowledgements resize the real overflow", async () => {
  const replies: Array<(value: typeof base) => void> = []
  vi.mocked(setPaneScroll).mockImplementation(() => new Promise((resolve) => replies.push(resolve)))
  const controllerRef: { current: PaneScrollController | null } = { current: null }
  render(<HerdrScrollbar sessionName="work" paneId="w1:p1" enabled canScroll={() => true} controllerRef={controllerRef} refreshRef={{ current: null }} viewportId="terminal" />)
  const bar = await screen.findByRole("scrollbar")
  const proxy = screen.getByTestId("herdr-scroll-proxy")
  act(() => controllerRef.current!.scroll(-10))
  expect(bar).toHaveAttribute("aria-valuenow", "90")
  expect(proxy.scrollTop).toBe(720)
  proxy.scrollTop = 400
  fireEvent.scroll(proxy)
  act(() => controllerRef.current!.scroll(-5))
  expect(bar).toHaveAttribute("aria-valuenow", "45")
  expect(setPaneScroll).toHaveBeenCalledTimes(1)
  await act(async () => replies[0]({ ...base, offsetFromBottom: 10, maxOffsetFromBottom: 200 }))
  expect(proxy.scrollHeight).toBe(1800)
  expect(bar).toHaveAttribute("aria-valuemax", "200")
  expect(bar).toHaveAttribute("aria-valuenow", "145")
  await waitFor(() => expect(setPaneScroll).toHaveBeenLastCalledWith("work", "w1:p1", 55, expect.any(AbortSignal)))
  fireEvent.scroll(proxy)
  expect(setPaneScroll).toHaveBeenCalledTimes(2)
  await act(async () => replies[1]({ ...base, offsetFromBottom: 55, maxOffsetFromBottom: 200 }))
})

it("reduces visible idle reads from the legacy 60/minute to 12/minute", async () => {
  vi.useFakeTimers()
  vi.spyOn(document, "visibilityState", "get").mockReturnValue("visible")
  const view = render(<HerdrScrollbar sessionName="work" paneId="w1:p1" enabled canScroll={() => true} refreshRef={{ current: null }} viewportId="terminal" />)
  await act(async () => {})
  expect(readPaneScroll).toHaveBeenCalledOnce()
  vi.mocked(readPaneScroll).mockClear()
  await act(async () => vi.advanceTimersByTimeAsync(60_000))
  expect(readPaneScroll).toHaveBeenCalledTimes(12)
  expect(60_000 / 1000).toBe(60) // Previous one-second fallback (excluding hydration).
  view.unmount()
  vi.mocked(readPaneScroll).mockClear()
  await act(async () => vi.advanceTimersByTimeAsync(60_000))
  expect(readPaneScroll).not.toHaveBeenCalled()
})

it("throttles output frames at 350ms and reads once after the last frame", async () => {
  vi.useFakeTimers()
  const controllerRef: { current: PaneScrollController | null } = { current: null }
  render(<HerdrScrollbar sessionName="work" paneId="w1:p1" enabled canScroll={() => true} controllerRef={controllerRef} refreshRef={{ current: null }} viewportId="terminal" />)
  await act(async () => {})
  vi.mocked(readPaneScroll).mockClear()
  for (let i = 0; i < 10; i++) {
    act(() => controllerRef.current!.frame())
    await act(async () => vi.advanceTimersByTimeAsync(100))
  }
  expect(readPaneScroll).toHaveBeenCalledTimes(2)
  await act(async () => vi.advanceTimersByTimeAsync(50))
  expect(readPaneScroll).toHaveBeenCalledTimes(3)
  await act(async () => vi.advanceTimersByTimeAsync(2000))
  expect(readPaneScroll).toHaveBeenCalledTimes(3)
  act(() => controllerRef.current!.scroll(-5))
  await act(async () => {})
  await act(async () => vi.advanceTimersByTimeAsync(150))
  expect(readPaneScroll).toHaveBeenCalledTimes(4)
})

it("stops fallback and frame reads while the document is hidden and refreshes on return", async () => {
  vi.useFakeTimers()
  const visibility = vi.spyOn(document, "visibilityState", "get").mockReturnValue("hidden")
  const controllerRef: { current: PaneScrollController | null } = { current: null }
  render(<HerdrScrollbar sessionName="work" paneId="w1:p1" enabled canScroll={() => true} controllerRef={controllerRef} refreshRef={{ current: null }} viewportId="terminal" />)
  act(() => controllerRef.current!.frame())
  await act(async () => vi.advanceTimersByTimeAsync(60_000))
  expect(readPaneScroll).not.toHaveBeenCalled()
  visibility.mockReturnValue("visible")
  await act(async () => document.dispatchEvent(new Event("visibilitychange")))
  expect(readPaneScroll).toHaveBeenCalledOnce()
  act(() => controllerRef.current!.frame())
  visibility.mockReturnValue("hidden")
  act(() => document.dispatchEvent(new Event("visibilitychange")))
  await act(async () => vi.advanceTimersByTimeAsync(60_000))
  expect(readPaneScroll).toHaveBeenCalledOnce()
})

it("discards queued gestures on hide so they cannot block refresh after becoming visible", async () => {
  vi.useFakeTimers()
  const visibility = vi.spyOn(document, "visibilityState", "get").mockReturnValue("visible")
  let acknowledge!: (value: typeof base) => void
  vi.mocked(setPaneScroll).mockImplementationOnce(() => new Promise(resolve => { acknowledge = resolve }))
  const controllerRef: { current: PaneScrollController | null } = { current: null }
  render(<HerdrScrollbar sessionName="work" paneId="w1:p1" enabled canScroll={() => true} controllerRef={controllerRef} refreshRef={{ current: null }} viewportId="terminal" />)
  await act(async () => {})
  act(() => { controllerRef.current!.move(10); controllerRef.current!.move(20) })
  expect(setPaneScroll).toHaveBeenCalledOnce()
  visibility.mockReturnValue("hidden")
  act(() => document.dispatchEvent(new Event("visibilitychange")))
  await act(async () => acknowledge({ ...base, offsetFromBottom: 10 }))
  await act(async () => vi.advanceTimersByTimeAsync(1000))
  expect(readPaneScroll).toHaveBeenCalledOnce()
  visibility.mockReturnValue("visible")
  await act(async () => document.dispatchEvent(new Event("visibilitychange")))
  expect(readPaneScroll).toHaveBeenCalledTimes(2)
  expect(setPaneScroll).toHaveBeenCalledOnce()
})

it("shows the proxy on the first controllable frame without waiting for fallback polling", async () => {
  vi.useFakeTimers()
  let allowed = false
  const controllerRef: { current: PaneScrollController | null } = { current: null }
  render(<HerdrScrollbar sessionName="work" paneId="w1:p1" enabled canScroll={() => allowed} controllerRef={controllerRef} refreshRef={{ current: null }} viewportId="terminal" />)
  expect(readPaneScroll).not.toHaveBeenCalled()
  allowed = true
  act(() => controllerRef.current!.frame())
  await act(async () => vi.advanceTimersByTimeAsync(1))
  expect(readPaneScroll).toHaveBeenCalledOnce()
  expect(screen.getByRole("scrollbar")).toBeInTheDocument()
})

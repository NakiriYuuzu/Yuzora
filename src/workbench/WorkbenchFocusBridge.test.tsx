import { afterEach, beforeEach, expect, it, vi } from "vitest"
import { act, cleanup, fireEvent, render } from "@testing-library/react"
import { WorkbenchFocusBridge } from "./WorkbenchFocusBridge"
const native = vi.hoisted(() => ({ enabled: false, callback: null as null | ((event: { payload: boolean }) => void), release: vi.fn() }))
vi.mock("@/lib/platform", () => ({ isTauri: () => native.enabled }))
vi.mock("@tauri-apps/api/window", () => ({ getCurrentWindow: () => ({
  onFocusChanged: async (callback: (event: { payload: boolean }) => void) => { native.callback = callback; return native.release }
}) }))
beforeEach(() => { vi.useFakeTimers(); native.enabled = false; native.callback = null; native.release.mockClear() })
afterEach(() => { cleanup(); vi.useRealTimers() })
it.each(["cm-content", "xterm-helper-textarea"])("restores last %s after application focus", (className) => {
  const view = render(<><WorkbenchFocusBridge /><textarea className={className} /></>)
  const target = view.container.querySelector("textarea")!
  target.focus()
  fireEvent.blur(window)
  target.blur()
  fireEvent.focus(window)
  act(() => vi.runAllTimers())
  expect(document.activeElement).toBe(target)
})
it("preserves dialog and ordinary input focus, ignores a hidden previous terminal", () => {
  const view = render(<><WorkbenchFocusBridge /><textarea className="xterm-helper-textarea" /><input /></>)
  const terminal = view.container.querySelector("textarea")!
  const input = view.container.querySelector("input")!
  terminal.focus()
  input.focus()
  fireEvent.focus(window)
  act(() => vi.runAllTimers())
  expect(document.activeElement).toBe(input)
  terminal.focus()
  terminal.blur()
  const dialog = document.createElement("div")
  dialog.setAttribute("aria-modal", "true")
  document.body.append(dialog)
  fireEvent.focus(window)
  act(() => vi.runAllTimers())
  expect(document.activeElement).not.toBe(terminal)
  dialog.remove()
  terminal.style.visibility = "hidden"
  fireEvent.focus(window)
  act(() => vi.runAllTimers())
  expect(document.activeElement).not.toBe(terminal)
})

it("handles native Tauri focus and unregisters its listener", async () => {
  native.enabled = true
  const view = render(<><WorkbenchFocusBridge /><textarea className="xterm-helper-textarea" /></>)
  await act(async () => { await Promise.resolve() })
  const terminal = view.container.querySelector("textarea")!
  terminal.focus()
  terminal.blur()
  act(() => { native.callback!({ payload: true }); vi.runAllTimers() })
  expect(document.activeElement).toBe(terminal)
  view.unmount()
  expect(native.release).toHaveBeenCalledOnce()
})

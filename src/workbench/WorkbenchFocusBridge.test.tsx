import { afterEach, beforeEach, expect, it, vi } from "vitest"
import { act, cleanup, fireEvent, render } from "@testing-library/react"
import { WorkbenchFocusBridge } from "./WorkbenchFocusBridge"
import { registerTerminalFocusTarget } from "@/terminal/terminalFocus"
import { useWorkspaceStore } from "@/state/workspaceStore"
const native = vi.hoisted(() => ({ enabled: false, callback: null as null | ((event: { payload: boolean }) => void), release: vi.fn(), focusWebview: vi.fn(), isFocused: vi.fn().mockResolvedValue(true) }))
vi.mock("@/lib/platform", () => ({ isTauri: () => native.enabled }))
vi.mock("@tauri-apps/api/window", () => ({ getCurrentWindow: () => ({
  onFocusChanged: async (callback: (event: { payload: boolean }) => void) => { native.callback = callback; return native.release },
  isFocused: native.isFocused
}) }))
vi.mock("@tauri-apps/api/webview", () => ({ getCurrentWebview: () => ({ setFocus: native.focusWebview }) }))
beforeEach(() => { vi.useFakeTimers(); native.enabled = false; native.callback = null; native.release.mockClear(); native.focusWebview.mockReset(); native.isFocused.mockResolvedValue(true) })
afterEach(() => { cleanup(); vi.restoreAllMocks(); vi.useRealTimers() })
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

it.each([0, 75])("restores keyboard input when its WebView activates %i ms after the native window", async (delay) => {
  native.enabled = true
  let webviewFocused = false
  vi.spyOn(document, "hasFocus").mockImplementation(() => webviewFocused)
  native.focusWebview.mockImplementation(async () => {
    setTimeout(() => { webviewFocused = true }, delay)
  })
  const path = "yuzora://herdr/focus-probe"
  useWorkspaceStore.setState({ groups: [{ tabs: [], activePath: path }], activeGroupIndex: 0 })
  const view = render(<><WorkbenchFocusBridge /><textarea className="xterm-helper-textarea" /></>)
  const terminal = view.container.querySelector("textarea")!
  const release = registerTerminalFocusTarget("probe", { pagePath: path, active: () => true,
    focus: () => { if (webviewFocused) terminal.focus() } })
  try {
    await act(async () => { await Promise.resolve() })
    await act(async () => { native.callback!({ payload: true }); await vi.runAllTimersAsync() })
    expect(document.activeElement).toBe(terminal)
    expect(native.focusWebview).toHaveBeenCalledOnce()
  } finally { release() }
})

it("reacquires native keyboard ownership even if document focus survived taskbar switching", async () => {
  native.enabled = true
  vi.spyOn(document, "hasFocus").mockReturnValue(true)
  let keyboardOwned = false
  native.focusWebview.mockImplementation(async () => {
    keyboardOwned = true
    // Reclaiming the WebView itself emits a DOM focus event. It must not
    // recursively request native focus again.
    fireEvent.focus(window)
  })
  const path = "yuzora://herdr/native-focus"
  useWorkspaceStore.setState({ groups: [{ tabs: [], activePath: path }], activeGroupIndex: 0 })
  const view = render(<><WorkbenchFocusBridge /><textarea className="xterm-helper-textarea" /></>)
  const terminal = view.container.querySelector("textarea")!
  const release = registerTerminalFocusTarget("native-focus", { pagePath: path, active: () => true,
    focus: () => { if (keyboardOwned) terminal.focus() } })
  try {
    await act(async () => { await Promise.resolve() })
    await act(async () => { native.callback!({ payload: true }); await vi.runAllTimersAsync() })
    expect(document.activeElement).toBe(terminal)
    expect(native.focusWebview).toHaveBeenCalledOnce()
  } finally { release() }
})

it("does not poll or focus a native window which is no longer active", async () => {
  native.enabled = true
  vi.spyOn(document, "hasFocus").mockReturnValue(false)
  native.isFocused.mockResolvedValue(false)
  const path = "yuzora://herdr/inactive-window"
  useWorkspaceStore.setState({ groups: [{ tabs: [], activePath: path }], activeGroupIndex: 0 })
  const focus = vi.fn()
  const release = registerTerminalFocusTarget("inactive-window", { pagePath: path, active: () => true, focus })
  render(<WorkbenchFocusBridge />)
  try {
    await act(async () => { await Promise.resolve() })
    await act(async () => { native.callback!({ payload: true }); await vi.runAllTimersAsync() })
    expect(native.focusWebview).not.toHaveBeenCalled()
    expect(focus).not.toHaveBeenCalled()
    expect(vi.getTimerCount()).toBe(0)
  } finally { release() }
})

it.each(["blur", "tab", "field", "dialog", "unmount"])("cancels pending native focus after %s", async (action) => {
  native.enabled = true
  vi.spyOn(document, "hasFocus").mockReturnValue(false)
  let resolveFocus!: (value: boolean) => void
  native.isFocused.mockImplementationOnce(() => new Promise(resolve => { resolveFocus = resolve }))
  const path = "yuzora://herdr/focus-probe"
  useWorkspaceStore.setState({ groups: [{ tabs: [], activePath: path }], activeGroupIndex: 0 })
  const focus = vi.fn()
  const release = registerTerminalFocusTarget("probe", { pagePath: path, active: () => true, focus })
  const view = render(<><WorkbenchFocusBridge /><input /></>)
  let dialog: HTMLElement | undefined
  try {
    await act(async () => { await Promise.resolve() })
    await act(async () => { native.callback!({ payload: true }); await vi.advanceTimersByTimeAsync(0) })
    if (action === "blur") act(() => native.callback!({ payload: false }))
    if (action === "tab") useWorkspaceStore.setState({ groups: [{ tabs: [], activePath: "/another-file" }] })
    if (action === "field") view.container.querySelector("input")!.focus()
    if (action === "dialog") {
      dialog = document.body.appendChild(document.createElement("div"))
      dialog.setAttribute("aria-modal", "true")
    }
    if (action === "unmount") view.unmount()
    await act(async () => { resolveFocus(true); await vi.runAllTimersAsync() })
    expect(native.focusWebview).not.toHaveBeenCalled()
    expect(focus).not.toHaveBeenCalled()
  } finally { dialog?.remove(); release() }
})

it("starts a fresh native focus attempt after a rapid leave and return", async () => {
  native.enabled = true
  let webviewFocused = false
  vi.spyOn(document, "hasFocus").mockImplementation(() => webviewFocused)
  native.focusWebview.mockImplementation(async () => { webviewFocused = true })
  let firstFocus!: (value: boolean) => void
  native.isFocused.mockImplementationOnce(() => new Promise(resolve => { firstFocus = resolve }))
  const path = "yuzora://herdr/focus-probe"
  useWorkspaceStore.setState({ groups: [{ tabs: [], activePath: path }], activeGroupIndex: 0 })
  const view = render(<><WorkbenchFocusBridge /><textarea className="xterm-helper-textarea" /></>)
  const terminal = view.container.querySelector("textarea")!
  const release = registerTerminalFocusTarget("probe", { pagePath: path, active: () => true,
    focus: () => { if (webviewFocused) terminal.focus() } })
  try {
    await act(async () => { await Promise.resolve() })
    await act(async () => { native.callback!({ payload: true }); await vi.advanceTimersByTimeAsync(0) })
    await act(async () => {
      native.callback!({ payload: false })
      native.callback!({ payload: true })
      await vi.advanceTimersByTimeAsync(0)
      firstFocus(true)
      await vi.runAllTimersAsync()
    })
    expect(document.activeElement).toBe(terminal)
  } finally { release() }
})

it("does not reclaim the WebView while a Browser tab owns the active group", async () => {
  native.enabled = true
  vi.spyOn(document, "hasFocus").mockReturnValue(false)
  useWorkspaceStore.setState({ groups: [{ tabs: [], activePath: "yuzora://browser/one" }], activeGroupIndex: 0 })
  const focus = vi.fn()
  const release = registerTerminalFocusTarget("hidden", { pagePath: "yuzora://herdr/one", active: () => false, focus })
  render(<WorkbenchFocusBridge />)
  try {
    await act(async () => { await Promise.resolve() })
    await act(async () => { native.callback!({ payload: true }); await vi.runAllTimersAsync() })
    expect(native.focusWebview).not.toHaveBeenCalled()
    expect(focus).not.toHaveBeenCalled()
  } finally { release() }
})

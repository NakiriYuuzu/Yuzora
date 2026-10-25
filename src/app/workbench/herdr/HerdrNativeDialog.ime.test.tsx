import { act, cleanup, render, screen, waitFor } from "@testing-library/react"
import { afterEach, beforeEach, expect, it, vi } from "vitest"
import type { Terminal } from "@xterm/xterm"
import HerdrNativeDialog from "./HerdrNativeDialog"

const mocks = vi.hoisted(() => ({
  invoke: vi.fn(), input: vi.fn(), release: vi.fn(), terminal: null as unknown,
}))
vi.mock("@/lib/herdrProvider", () => ({ invokeHerdr: mocks.invoke }))
vi.mock("@/lib/herdrFeatures", () => ({ herdrFeature: vi.fn() }))
vi.mock("@/lib/herdrIpc", () => ({
  herdrTerminalInput: mocks.input,
  herdrTerminalRelease: mocks.release,
  herdrTerminalResize: vi.fn(async () => undefined),
  herdrPaneFocus: vi.fn(async () => undefined),
}))
vi.mock("@/state/herdrStore", () => ({ useHerdrStore: { getState: () => ({ bootstrap: vi.fn() }) } }))
vi.mock("@tauri-apps/plugin-clipboard-manager", () => ({ readText: vi.fn(), writeText: vi.fn() }))
vi.mock("@xterm/addon-fit", () => ({ FitAddon: class { activate() {} fit() {} dispose() {} } }))
// Keep real xterm input/parser behavior. Canvas image rendering is unrelated
// to these recorded IME event sequences and is unavailable in jsdom.
vi.mock("@/terminal/kittyRenderer", () => ({ installKittyRenderer: (term: Terminal) => {
  mocks.terminal = term
  return { write: (data: Uint8Array) => new Promise<void>(resolve => term.write(data, resolve)), dispose: () => undefined }
} }))

const nextTask = () => new Promise<void>(resolve => window.setTimeout(resolve, 0))
function insertText(textarea: HTMLTextAreaElement, data: string) {
  textarea.dispatchEvent(new InputEvent("beforeinput", { data, inputType: "insertText", composed: true, cancelable: true }))
  textarea.value += data
  textarea.selectionStart = textarea.selectionEnd = textarea.value.length
  textarea.dispatchEvent(new InputEvent("input", { data, inputType: "insertText", composed: true }))
}
function key(textarea: HTMLTextAreaElement, type: "keydown" | "keyup", value: string, code: string, keyCode: number) {
  textarea.dispatchEvent(new KeyboardEvent(type, { key: value, code, keyCode, bubbles: true, cancelable: true }))
}
async function open() {
  const mounted = render(<HerdrNativeDialog selection={{ sessionName: "owned-ime-fixture" }} />)
  await waitFor(() => expect(mocks.invoke).toHaveBeenCalledOnce())
  await waitFor(() => expect(screen.queryByRole("status")).toBeNull())
  const term = mocks.terminal as Terminal
  if (!term.textarea) throw new Error("Real xterm input missing")
  return { ...mounted, term, textarea: term.textarea }
}
const sent = () => mocks.input.mock.calls.map(([, text]) => text)

beforeEach(() => {
  mocks.invoke.mockReset().mockResolvedValue({ sessionId: "owned-native-ime" })
  mocks.input.mockReset().mockResolvedValue(undefined)
  mocks.release.mockReset().mockResolvedValue(undefined)
  mocks.terminal = null
})
afterEach(() => { cleanup() })

it("preserves recorded WKWebView punctuation inserted before the previous keyup in the native dialog", async () => {
  const { textarea } = await open()
  await act(async () => {
    insertText(textarea, "你")
    key(textarea, "keydown", "你", "KeyN", 229)
    await nextTask()
    insertText(textarea, "，")
    key(textarea, "keydown", "，", "Comma", 229)
    await nextTask()
    key(textarea, "keyup", "n", "KeyN", 78)
    key(textarea, "keyup", "，", "Comma", 188)
    await nextTask()
  })
  expect(sent()).toEqual(["你", "，"])
})

it("preserves a complete Windows TSF commit after replacing the native dialog textarea value", async () => {
  const { textarea } = await open()
  await act(async () => {
    textarea.value = "   "
    textarea.selectionStart = textarea.selectionEnd = 3
    textarea.dispatchEvent(new CompositionEvent("compositionstart", { data: "" }))
    textarea.dispatchEvent(new CompositionEvent("compositionupdate", { data: "n" }))
    textarea.value = "   n"
    textarea.selectionStart = textarea.selectionEnd = 4
    await nextTask()
    textarea.selectionStart = 0
    textarea.selectionEnd = 4
    textarea.dispatchEvent(new CompositionEvent("compositionupdate", { data: "ni" }))
    textarea.value = "ni"
    textarea.selectionStart = textarea.selectionEnd = 2
    await nextTask()
    textarea.dispatchEvent(new CompositionEvent("compositionend", { data: "你是" }))
    textarea.value = "你是"
    textarea.selectionStart = textarea.selectionEnd = 2
    await nextTask()
    await nextTask()
  })
  expect(sent()).toEqual(["你是"])
})

it("does not duplicate a native dialog ASCII keypress followed by insertText", async () => {
  const { textarea } = await open()
  await act(async () => {
    key(textarea, "keydown", "A", "KeyA", 65)
    textarea.dispatchEvent(new KeyboardEvent("keypress", { key: "A", code: "KeyA", keyCode: 65, charCode: 65, bubbles: true, cancelable: true }))
    insertText(textarea, "A")
    key(textarea, "keyup", "A", "KeyA", 65)
    await nextTask()
    await nextTask()
  })
  expect(sent()).toEqual(["A"])
})

it("preserves native dialog protocol replies and stops input after unmount", async () => {
  const { term, textarea, unmount } = await open()
  await act(async () => { await new Promise<void>(resolve => term.write("\x1b[5n", resolve)) })
  expect(sent()).toEqual(["\x1b[0n"])
  unmount()
  await act(async () => { insertText(textarea, "late input"); await nextTask(); await nextTask() })
  expect(sent()).toEqual(["\x1b[0n"])
  expect(mocks.release).toHaveBeenCalledExactlyOnceWith("owned-native-ime")
})

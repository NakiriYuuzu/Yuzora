import { afterEach, beforeEach, expect, it, vi } from "vitest"
import { act, cleanup, renderHook } from "@testing-library/react"
import { cycleHoldModifiers, useAgentHotkeys } from "./useAgentHotkeys"
import { useAgentMruStore } from "@/state/agentMruStore"
import { useKeyboardSettingsStore } from "@/state/keyboardSettingsStore"

let terminalInput: HTMLTextAreaElement
let terminalKey: ReturnType<typeof vi.fn<(event: Event) => void>>
beforeEach(() => {
  vi.spyOn(navigator, "userAgent", "get").mockReturnValue("Windows")
  useKeyboardSettingsStore.setState({ overrides: {} })
  useAgentMruStore.setState({ keys: [] })
  const terminal = document.body.appendChild(document.createElement("div"))
  terminal.className = "xterm"
  terminalInput = terminal.appendChild(document.createElement("textarea"))
  terminalKey = vi.fn<(event: Event) => void>()
  terminalInput.addEventListener("keydown", terminalKey)
})
afterEach(() => {
  cleanup()
  document.body.innerHTML = ""
  vi.restoreAllMocks()
})

const down = (init: KeyboardEventInit) => act(() => { terminalInput.dispatchEvent(new KeyboardEvent("keydown", { bubbles: true, cancelable: true, ...init })) })
const up = (init: KeyboardEventInit) => act(() => { terminalInput.dispatchEvent(new KeyboardEvent("keyup", { bubbles: true, cancelable: true, ...init })) })
function setup(agents = ["a", "b", "c"]) {
  const activate = vi.fn()
  const hook = renderHook(() => useAgentHotkeys({ agents, keyOf: (agent) => agent, activate }))
  return { activate, hook }
}

it("jumps to the Nth Agent from inside a terminal and ignores missing positions", () => {
  const { activate } = setup()
  down({ key: "2", code: "Digit2", altKey: true })
  expect(activate).toHaveBeenCalledWith("b")
  expect(terminalKey).not.toHaveBeenCalled()
  down({ key: "4", code: "Digit4", altKey: true })
  expect(activate).toHaveBeenCalledTimes(1)
})

it("matches macOS Option-digit jumps by physical key", () => {
  vi.spyOn(navigator, "userAgent", "get").mockReturnValue("Macintosh")
  const { activate } = setup()
  down({ key: "\u00a1", code: "Digit1", altKey: true })
  expect(activate).toHaveBeenCalledWith("a")
})

it("tracks the held Alt key for number badges", () => {
  const { hook } = setup()
  down({ key: "Alt", code: "AltLeft", altKey: true })
  expect(hook.result.current.altHeld).toBe(true)
  up({ key: "Alt", code: "AltLeft" })
  expect(hook.result.current.altHeld).toBe(false)
})

it("switches to the previous Agent on a single Alt+` tap, most recent first", () => {
  useAgentMruStore.setState({ keys: ["b", "c"] })
  const { activate, hook } = setup()
  down({ key: "`", code: "Backquote", altKey: true })
  expect(hook.result.current.switcher?.items).toEqual(["b", "c", "a"])
  expect(hook.result.current.switcher?.index).toBe(1)
  up({ key: "Alt", code: "AltLeft" })
  expect(activate).toHaveBeenCalledWith("c")
  expect(hook.result.current.switcher).toBeNull()
})

it("moves forward on repeated presses and backwards with Shift, wrapping around", () => {
  useAgentMruStore.setState({ keys: ["b", "c"] })
  const { activate, hook } = setup()
  down({ key: "`", code: "Backquote", altKey: true })
  down({ key: "`", code: "Backquote", altKey: true })
  expect(hook.result.current.switcher?.index).toBe(2)
  down({ key: "~", code: "Backquote", altKey: true, shiftKey: true })
  down({ key: "~", code: "Backquote", altKey: true, shiftKey: true })
  down({ key: "~", code: "Backquote", altKey: true, shiftKey: true })
  expect(hook.result.current.switcher?.index).toBe(2)
  up({ key: "Shift", code: "ShiftLeft", altKey: true })
  expect(activate).not.toHaveBeenCalled()
  up({ key: "Alt", code: "AltLeft" })
  expect(activate).toHaveBeenCalledWith("a")
})

it("cancels on Escape or window blur without switching", () => {
  const { activate, hook } = setup()
  down({ key: "`", code: "Backquote", altKey: true })
  down({ key: "Escape", code: "Escape", altKey: true })
  expect(hook.result.current.switcher).toBeNull()
  down({ key: "`", code: "Backquote", altKey: true })
  act(() => { window.dispatchEvent(new Event("blur")) })
  expect(hook.result.current.switcher).toBeNull()
  up({ key: "Alt", code: "AltLeft" })
  expect(activate).not.toHaveBeenCalled()
})

it("does not open for fewer than two Agents or while a dialog is open", () => {
  const single = setup(["only"])
  down({ key: "`", code: "Backquote", altKey: true })
  expect(single.hook.result.current.switcher).toBeNull()
  single.hook.unmount()
  const { hook } = setup()
  const dialog = document.body.appendChild(document.createElement("div"))
  dialog.setAttribute("aria-modal", "true")
  down({ key: "`", code: "Backquote", altKey: true })
  expect(hook.result.current.switcher).toBeNull()
})

it("uses Option+Tab by default on macOS", () => {
  vi.spyOn(navigator, "userAgent", "get").mockReturnValue("Macintosh")
  const { hook } = setup()
  down({ key: "Tab", code: "Tab", altKey: true })
  expect(hook.result.current.switcher?.index).toBe(1)
})

it("derives the held modifiers from the binding", () => {
  expect(cycleHoldModifiers("Alt+`", false)).toEqual(["altKey"])
  expect(cycleHoldModifiers("Mod+Shift+J", true)).toEqual(["metaKey"])
  expect(cycleHoldModifiers("Mod+J", false)).toEqual(["ctrlKey"])
  expect(cycleHoldModifiers("Shift+F5", false)).toEqual(["shiftKey"])
})

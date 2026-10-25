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

it("keeps the switcher open while the modifier of the opening binding is held", () => {
  // Reverse cycle rebound to a different modifier than forward cycle.
  useKeyboardSettingsStore.setState({ overrides: { agentCycleNext: "Ctrl+J", agentCyclePrevious: "Alt+K" } })
  useAgentMruStore.setState({ keys: ["b", "c"] })
  const { activate, hook } = setup()
  down({ key: "k", code: "KeyK", altKey: true })
  up({ key: "k", code: "KeyK", altKey: true })
  expect(activate).not.toHaveBeenCalled()
  down({ key: "k", code: "KeyK", altKey: true })
  expect(hook.result.current.switcher?.index).toBe(1)
  up({ key: "Alt", code: "AltLeft" })
  expect(activate).toHaveBeenCalledWith("c")
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

it.each(["release", "click"])("commits the latest same-key Agent after rerender via %s", method => {
  const a = { id: "a", session: "one", title: "A", pane: "a" }
  const b = { id: "b", session: "one", title: "B", pane: "old" }
  const c = { id: "c", session: "one", title: "C", pane: "c" }
  const keyOf = (agent: typeof a) => JSON.stringify([agent.session, agent.id])
  const originalActivate = vi.fn(), latestActivate = vi.fn()
  const hook = renderHook(({ agents, activate }) => useAgentHotkeys({ agents, keyOf, activate }), {
    initialProps: { agents: [a, b, c], activate: originalActivate }
  })
  down({ key: "`", code: "Backquote", altKey: true })
  const replacement = { ...b, title: "Renamed", pane: "new" }
  useAgentMruStore.setState({ keys: [keyOf(c), keyOf(a)] })
  hook.rerender({ agents: [c, replacement, a], activate: latestActivate })
  expect(hook.result.current.switcher?.items.map(keyOf)).toEqual([keyOf(a), keyOf(b), keyOf(c)])
  expect(hook.result.current.switcher?.index).toBe(1)
  if (method === "click") act(() => hook.result.current.commit(1))
  else up({ key: "Alt", code: "AltLeft" })
  expect(originalActivate).not.toHaveBeenCalled()
  expect(latestActivate).toHaveBeenCalledExactlyOnceWith(replacement)
  expect(hook.result.current.switcher).toBeNull()
})

it.each(["removed", "all-removed", "same-name", "other-session"])("dismisses a %s Agent selection without activating a replacement", variant => {
  const a = { id: "a", session: "one", name: "A" }
  const b = { id: "b", session: "one", name: "B" }
  const activate = vi.fn(), keyOf = (agent: typeof a) => JSON.stringify([agent.session, agent.id])
  const hook = renderHook(({ agents }) => useAgentHotkeys({ agents, keyOf, activate }), { initialProps: { agents: [a, b] } })
  down({ key: "`", code: "Backquote", altKey: true })
  const agents = variant === "all-removed" ? [] : variant === "removed" ? [a]
    : [a, variant === "same-name" ? { ...b, id: "new-id" } : { ...b, session: "two" }]
  hook.rerender({ agents })
  up({ key: "Alt", code: "AltLeft" })
  expect(activate).not.toHaveBeenCalled()
  expect(hook.result.current.switcher).toBeNull()
})

it.each([
  { current: "b", reverse: false, expected: "a", order: ["b", "a", "c"] },
  { current: "b", reverse: true, expected: "c", order: ["b", "a", "c"] },
  { current: null, reverse: false, expected: "a", order: ["a", "b", "c"] },
  { current: null, reverse: true, expected: "c", order: ["a", "b", "c"] },
  { current: "outside-list", reverse: false, expected: "a", order: ["a", "b", "c"] },
  { current: undefined, reverse: false, expected: "b", order: ["a", "b", "c"] },
])("starts cycling relative to current=$current reverse=$reverse without writing MRU", ({ current, reverse, expected, order }) => {
  const activate = vi.fn()
  const hook = renderHook(() => useAgentHotkeys({ agents: ["a", "b", "c"], keyOf: agent => agent, activate, getCurrentKey: () => current }))
  down({ key: reverse ? "~" : "`", code: "Backquote", altKey: true, shiftKey: reverse })
  expect(hook.result.current.switcher?.items).toEqual(order)
  expect(useAgentMruStore.getState().keys).toEqual([])
  up({ key: "Alt", code: "AltLeft" })
  expect(activate).toHaveBeenCalledExactlyOnceWith(expected)
})

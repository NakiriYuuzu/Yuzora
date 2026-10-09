import { afterEach, expect, it } from "vitest"

import { registerTerminalDropTarget, terminalDropTargetAt, type TerminalDropTarget } from "./terminalDropTargets"

const target = (): TerminalDropTarget => ({
  scope: "main",
  canWrite: () => true,
  paste: async () => undefined,
  focus: () => undefined,
})

afterEach(() => {
  document.body.innerHTML = ""
})

it("resolves a registered target from any descendant of its leaf", () => {
  document.body.innerHTML = `<div data-attachment-key="k1"><span id="inner"></span></div><p id="out"></p>`
  const entry = target()
  const unregister = registerTerminalDropTarget("k1", entry)

  expect(terminalDropTargetAt(document.getElementById("inner"))).toBe(entry)
  expect(terminalDropTargetAt(document.getElementById("out"))).toBeNull()
  expect(terminalDropTargetAt(null)).toBeNull()
  unregister()
})

it("stops resolving after unregister and ignores a stale disposer", () => {
  document.body.innerHTML = `<div data-attachment-key="k2"></div>`
  const leaf = document.querySelector("[data-attachment-key]")
  const first = target()
  const unregisterFirst = registerTerminalDropTarget("k2", first)
  const second = target()
  const unregisterSecond = registerTerminalDropTarget("k2", second)

  unregisterFirst()
  expect(terminalDropTargetAt(leaf)).toBe(second)
  unregisterSecond()
  expect(terminalDropTargetAt(leaf)).toBeNull()
})

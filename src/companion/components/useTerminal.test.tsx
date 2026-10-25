import { act, cleanup, renderHook, waitFor } from "@testing-library/react"
import type { ITerminalOptions } from "@xterm/xterm"
import type { ReactNode } from "react"
import { I18nextProvider } from "react-i18next"
import { afterEach, beforeEach, expect, it, vi } from "vitest"
import { i18n } from "../i18n"

const xterm = vi.hoisted(() => ({ options: [] as unknown[], addons: 0 }))
vi.mock("@xterm/xterm", () => ({
  Terminal: class {
    element = null
    constructor(options: unknown) {
      xterm.options.push(options)
    }
    loadAddon() {
      xterm.addons += 1
    }
    open() {}
    dispose() {}
  },
}))
import { useTerminal } from "./useTerminal"

const wrapper = ({ children }: { children: ReactNode }) => <I18nextProvider i18n={i18n}>{children}</I18nextProvider>

async function linkHandler() {
  const view = renderHook(() => useTerminal(), { wrapper })
  act(() => view.result.current.mount(document.createElement("div")))
  await waitFor(() => expect(view.result.current.terminal).not.toBeNull())
  const options = xterm.options.at(-1) as ITerminalOptions
  return options.linkHandler!
}

beforeEach(async () => {
  await i18n.changeLanguage("en")
  xterm.options = []
  xterm.addons = 0
  Object.defineProperty(document, "fonts", { configurable: true, value: { load: () => Promise.resolve([]) } })
})
afterEach(() => {
  cleanup()
  vi.restoreAllMocks()
})

it("opens a terminal link only after the user confirms it, in a detached window", async () => {
  const confirm = vi.spyOn(window, "confirm").mockReturnValue(false)
  const open = vi.spyOn(window, "open").mockReturnValue(null)
  const handler = await linkHandler()
  expect(handler.allowNonHttpProtocols).toBe(false)

  handler.activate(new MouseEvent("click"), "https://evil.example/phish", { start: { x: 1, y: 1 }, end: { x: 2, y: 1 } })
  expect(confirm).toHaveBeenCalledTimes(1)
  expect(confirm.mock.calls[0][0]).toContain("https://evil.example/phish") // the user sees the real target
  expect(open).not.toHaveBeenCalled() // declined

  confirm.mockReturnValue(true)
  handler.activate(new MouseEvent("click"), "https://example.com/ok", { start: { x: 1, y: 1 }, end: { x: 2, y: 1 } })
  expect(open).toHaveBeenCalledTimes(1)
  expect(open).toHaveBeenCalledWith("https://example.com/ok", "_blank", "noopener,noreferrer")
})

it("never asks about, let alone opens, a non-http(s) terminal link", async () => {
  const confirm = vi.spyOn(window, "confirm").mockReturnValue(true)
  const open = vi.spyOn(window, "open").mockReturnValue(null)
  const handler = await linkHandler()
  for (const uri of ["javascript:alert(1)", "data:text/html,<script>1</script>", "file:///etc/passwd", "ssh://host", "vbscript:x", "//evil.example", "tel:1", " https://example.com", "x https://example.com"]) {
    handler.activate(new MouseEvent("click"), uri, { start: { x: 1, y: 1 }, end: { x: 2, y: 1 } })
  }
  expect(confirm).not.toHaveBeenCalled()
  expect(open).not.toHaveBeenCalled()
})

it("loads no xterm addon: no clipboard, no automatic URL detection", async () => {
  await linkHandler()
  expect(xterm.addons).toBe(0)
  expect((xterm.options.at(-1) as ITerminalOptions).disableStdin).toBe(true)
})

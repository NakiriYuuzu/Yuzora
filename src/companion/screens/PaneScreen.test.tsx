import { act, cleanup, render, waitFor } from "@testing-library/react"
import type { Terminal } from "@xterm/xterm"
import { I18nextProvider } from "react-i18next"
import { afterEach, beforeEach, expect, it, vi } from "vitest"
import { i18n } from "../i18n"
import type { ControlOpen } from "../lib/protocol"
import { useCompanion } from "../store"

const client = vi.hoisted(() => ({
  call: vi.fn(),
  listen: vi.fn(() => () => undefined),
  observe: vi.fn(),
}))
vi.mock("../client", () => client)
const terminal = vi.hoisted(() => ({ value: { reset() {}, resize() {}, write() {} } as unknown as Terminal }))
vi.mock("../components/useTerminal", () => ({ useTerminal: () => ({ terminal: terminal.value, mount: () => undefined }) }))
vi.mock("../components/TerminalFrame", () => ({ TerminalFrame: () => null }))
const panel = vi.hoisted(() => ({ onControl: null as null | ((control: ControlOpen | null) => void) }))
vi.mock("./ControlPanel", () => ({
  ControlPanel: ({ onControl }: { onControl: (control: ControlOpen | null) => void }) => {
    panel.onControl = onControl
    return null
  },
}))
import { PaneScreen } from "./PaneScreen"

const opened: ControlOpen = { stream: 9, cols: 80, rows: 24, until: "2026-10-18T13:00:00Z", target: "w:p" }

beforeEach(async () => {
  await i18n.changeLanguage("en")
  client.call.mockReset().mockResolvedValue({})
  client.listen.mockClear()
  client.observe.mockReset().mockResolvedValue({ stream: 3, cols: 80, rows: 24 })
  useCompanion.setState({ socket: "ready", epoch: 1, snapshots: {} })
})
afterEach(cleanup)

const mount = () => render(<I18nextProvider i18n={i18n}><PaneScreen session="work" pane="w:p" /></I18nextProvider>)

it("releases the control stream when the screen goes away", async () => {
  const view = mount()
  await waitFor(() => expect(panel.onControl).not.toBeNull())
  act(() => panel.onControl!(opened))
  await waitFor(() => expect(client.listen).toHaveBeenCalledWith(9, expect.anything()))
  expect(client.call).not.toHaveBeenCalledWith("terminal.release", expect.anything())
  view.unmount()
  expect(client.call).toHaveBeenCalledWith("terminal.release", { stream: 9 })
})

it("releases nothing when no control stream is open", async () => {
  const view = mount()
  await waitFor(() => expect(client.observe).toHaveBeenCalled())
  view.unmount()
  expect(client.call).not.toHaveBeenCalledWith("terminal.release", expect.anything())
})

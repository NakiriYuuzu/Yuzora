import { fireEvent, render, screen, waitFor, within } from "@testing-library/react"
import type { Terminal } from "@xterm/xterm"
import { I18nextProvider } from "react-i18next"
import { beforeEach, expect, it, vi } from "vitest"
import { i18n } from "../i18n"
import type { AgentRow } from "../lib/agents"
import type { ControlOpen } from "../lib/protocol"
import { RpcError } from "../lib/rpc"
import { useCompanion } from "../store"

const client = vi.hoisted(() => ({ call: vi.fn() }))
vi.mock("../client", () => ({ call: client.call }))
// The passkey ceremony has its own path; here a tap stands in for it.
vi.mock("../components/ElevateDialog", () => ({
  ElevateDialog: ({ title, onElevated, onCancel }: { title: string; onElevated: () => void; onCancel: () => void }) => (
    <div role="dialog" aria-label={title}>
      <button onClick={onElevated}>passkey ok</button>
      <button onClick={onCancel}>passkey cancel</button>
    </div>
  ),
}))
import { ControlPanel } from "./ControlPanel"

const agent = (status: AgentRow["status"]): AgentRow => ({
  session: "work", paneId: "w:p", name: "pi", kind: "pi", status, tabLabel: null, paneLabel: null, hint: null,
})
const opened: ControlOpen = { stream: 7, cols: 80, rows: 24, until: "2026-10-18T13:00:00Z", target: "w:p" }
const terminal = (lines: string[]) => ({
  rows: lines.length,
  buffer: { active: { viewportY: 0, getLine: (row: number) => ({ translateToString: () => lines[row] ?? "" }) } },
  modes: { applicationCursorKeysMode: false },
}) as unknown as Terminal

function renderPanel(props: Partial<Parameters<typeof ControlPanel>[0]> = {}) {
  const onControl = vi.fn()
  const onConflictHandled = vi.fn()
  render(
    <I18nextProvider i18n={i18n}>
      <ControlPanel session="work" pane="w:p" agent={agent("idle")} terminal={null} control={null}
        onControl={onControl} conflict={false} onConflictHandled={onConflictHandled} {...props} />
    </I18nextProvider>,
  )
  return { onControl, onConflictHandled }
}

beforeEach(async () => {
  await i18n.changeLanguage("en")
  client.call.mockReset()
  useCompanion.setState({
    socket: "ready",
    offset: 0,
    notice: null,
    elevation: { scope: "control", until: new Date(Date.now() + 10 * 60_000).toISOString(), at: Date.now() },
  })
})

it("asks for a fresh passkey before taking over a pane owned elsewhere", async () => {
  client.call.mockRejectedValueOnce(new RpcError("owned-elsewhere")).mockResolvedValueOnce(opened)
  const { onControl } = renderPanel()
  fireEvent.click(screen.getByRole("button", { name: "Direct control" }))
  fireEvent.click(within(screen.getByRole("alertdialog")).getByRole("button", { name: "Direct control" }))
  const takeover = await screen.findByRole("dialog", { name: "Take over this pane" })
  expect(client.call.mock.calls).toEqual([["terminal.control", { session: "work", target: "w:p", takeover: false, confirmResize: true }]])
  fireEvent.click(within(takeover).getByRole("button", { name: "passkey ok" }))
  await waitFor(() => expect(onControl).toHaveBeenCalledWith(opened))
  expect(client.call.mock.calls[1]).toEqual(["terminal.control", { session: "work", target: "w:p", takeover: true, confirmResize: true }])
})

it("turns a stream closed as owned elsewhere into the same confirmation, and cancelling takes nothing", () => {
  const { onConflictHandled } = renderPanel({ conflict: true })
  fireEvent.click(within(screen.getByRole("dialog", { name: "Take over this pane" })).getByRole("button", { name: "passkey cancel" }))
  expect(onConflictHandled).toHaveBeenCalled()
  expect(client.call).not.toHaveBeenCalled()
})

it("shows a blocked agent's request and waits for a second tap before any key", async () => {
  client.call.mockResolvedValue({})
  renderPanel({ agent: agent("blocked"), terminal: terminal(["Agent wants to run: rm -rf build", "Allow? [y/N]", ""]) })
  expect(screen.queryByRole("textbox")).toBeNull()
  expect(screen.getByText(/waiting for interactive input/)).toBeInTheDocument()
  fireEvent.click(screen.getByRole("button", { name: "Enter" }))
  const dialog = screen.getByRole("alertdialog")
  expect(within(dialog).getByText(/Allow\? \[y\/N\]/)).toBeInTheDocument()
  expect(client.call).not.toHaveBeenCalled()
  fireEvent.click(within(dialog).getByRole("button", { name: "Send anyway" }))
  await waitFor(() => expect(client.call).toHaveBeenCalledWith("pane.sendKeys", { session: "work", target: "w:p", keys: ["Enter"] }))
})

it("never resends a write whose outcome is unknown", async () => {
  client.call.mockRejectedValue(new RpcError("write-outcome-unknown"))
  renderPanel()
  fireEvent.change(screen.getByRole("textbox"), { target: { value: "hello" } })
  fireEvent.click(screen.getByRole("button", { name: "Send to agent" }))
  await waitFor(() => expect(useCompanion.getState().notice?.tone).toBe("warning"))
  expect(client.call).toHaveBeenCalledTimes(1)
  expect(client.call).toHaveBeenCalledWith("agent.prompt", { session: "work", target: "w:p", text: "hello" })
})

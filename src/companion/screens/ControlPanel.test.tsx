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
  const view = render(
    <I18nextProvider i18n={i18n}>
      <ControlPanel session="work" pane="w:p" agent={agent("idle")} terminal={null} control={null}
        onControl={onControl} conflict={false} onConflictHandled={onConflictHandled} {...props} />
    </I18nextProvider>,
  )
  return { onControl, onConflictHandled, unmount: view.unmount }
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

it("uses the shared checkbox for appending Enter", () => {
  renderPanel({ agent: undefined, control: opened })
  const box = screen.getByRole("checkbox", { name: "Press Enter after" })
  expect(box).toHaveAttribute("data-slot", "checkbox")
  expect(box).toHaveAttribute("aria-checked", "true")
  fireEvent.click(box)
  expect(box).toHaveAttribute("aria-checked", "false")
})

it("picks the input target with the shared radio group: one tab stop, arrow keys move the choice", async () => {
  renderPanel({ control: opened })
  const group = screen.getByRole("radiogroup", { name: "Send to" })
  expect(group).toHaveAttribute("data-slot", "radio-group")
  const toAgent = within(group).getByRole("radio", { name: "Agent" })
  const toTerminal = within(group).getByRole("radio", { name: "Terminal" })
  expect(toAgent).toHaveAttribute("aria-checked", "true")
  // The group is one tab stop: the option not chosen is reached with the arrow keys.
  expect(toTerminal.tabIndex).toBe(-1)
  fireEvent.click(toTerminal)
  expect(toTerminal).toHaveAttribute("aria-checked", "true")
  expect(screen.getByRole("textbox", { name: "Type into the terminal" })).toBeInTheDocument()
  toTerminal.focus()
  fireEvent.keyDown(toTerminal, { key: "ArrowLeft" })
  await waitFor(() => expect(toAgent).toHaveAttribute("aria-checked", "true"))
  expect(screen.getByRole("textbox", { name: "Message the agent" })).toBeInTheDocument()
})

it("scrolls the approval preview in a ScrollArea rather than a bare overflow", () => {
  renderPanel({ agent: agent("blocked"), terminal: terminal(["Allow? [y/N]"]) })
  fireEvent.click(screen.getByRole("button", { name: "Enter" }))
  const dialog = screen.getByRole("alertdialog")
  const area = dialog.querySelector('[data-slot="scroll-area"]')
  expect(area).not.toBeNull()
  expect(area).toHaveTextContent("Allow? [y/N]")
  expect(dialog.querySelector(".overflow-auto")).toBeNull()
})

it("releases a control stream that opens after the panel went away", async () => {
  let resolveOpen: (value: ControlOpen) => void = () => undefined
  client.call.mockImplementation((method: string) =>
    method === "terminal.control" ? new Promise<ControlOpen>((done) => { resolveOpen = done }) : Promise.resolve({}))
  const { onControl, unmount } = renderPanel()
  fireEvent.click(screen.getByRole("button", { name: "Direct control" }))
  fireEvent.click(within(screen.getByRole("alertdialog")).getByRole("button", { name: "Direct control" }))
  await waitFor(() => expect(client.call).toHaveBeenCalledWith("terminal.control", expect.anything()))
  unmount()
  resolveOpen(opened)
  await waitFor(() => expect(client.call).toHaveBeenCalledWith("terminal.release", { stream: 7 }))
  expect(client.call.mock.calls.filter(([method]) => method === "terminal.release")).toHaveLength(1)
  expect(onControl).not.toHaveBeenCalled()
})

it("releases a takeover stream that opens after the panel went away", async () => {
  let resolveOpen: (value: ControlOpen) => void = () => undefined
  client.call.mockImplementation((method: string) =>
    method === "terminal.control" ? new Promise<ControlOpen>((done) => { resolveOpen = done }) : Promise.resolve({}))
  const { unmount } = renderPanel({ conflict: true })
  fireEvent.click(within(screen.getByRole("dialog", { name: "Take over this pane" })).getByRole("button", { name: "passkey ok" }))
  await waitFor(() => expect(client.call).toHaveBeenCalledWith("terminal.control", expect.objectContaining({ takeover: true })))
  unmount()
  resolveOpen(opened)
  await waitFor(() => expect(client.call).toHaveBeenCalledWith("terminal.release", { stream: 7 }))
})

it("shows the whole pending request in the approval dialog, not a truncated preview", () => {
  const long = `Run: ${"x".repeat(600)} --force`
  const lines = Array.from({ length: 60 }, (_, index) => `line ${index + 1}`)
  lines.splice(30, 0, long)
  // Blank rows after the request are trimmed; the ones inside it are kept.
  renderPanel({ agent: agent("blocked"), terminal: terminal([...lines.slice(0, 20), "", ...lines.slice(20), "", ""]) })
  fireEvent.click(screen.getByRole("button", { name: "Enter" }))
  const dialog = screen.getByRole("alertdialog")
  const preview = dialog.querySelector("pre")!
  expect(preview.textContent).toBe([...lines.slice(0, 20), "", ...lines.slice(20)].join("\n"))
  expect(preview.textContent).toContain(long)
  const area = dialog.querySelector('[data-slot="scroll-area"]')!
  expect(area).toContainElement(preview)
  // The preview is scrollable (bounded height, both axes) rather than clamped or ellipsised.
  expect(area.className).toMatch(/max-h-/)
  expect(preview.className).toContain("whitespace-pre")
  expect(preview.className).not.toMatch(/truncate|line-clamp|text-ellipsis/)
  expect(client.call).not.toHaveBeenCalled()
})

it("says the working directory is not shared, and an empty screen is shown as empty", () => {
  renderPanel({ agent: agent("blocked"), terminal: terminal(["", "  ", ""]) })
  fireEvent.click(screen.getByRole("button", { name: "Enter" }))
  const dialog = screen.getByRole("alertdialog")
  // The protocol carries no cwd (snapshot panes have id/tab/status/label/layout
  // only), so the dialog must tell the user it cannot show it.
  expect(within(dialog).getByText(/does not share the working directory/)).toBeInTheDocument()
  expect(within(dialog).getByText("(The screen is empty.)")).toBeInTheDocument()
})

it("sends nothing for a blocked agent until the second tap, and cancelling sends nothing at all", () => {
  renderPanel({ agent: agent("blocked"), terminal: terminal(["Allow? [y/N]"]), control: opened })
  fireEvent.change(screen.getByRole("textbox"), { target: { value: "y" } })
  fireEvent.click(screen.getByRole("button", { name: "Send" }))
  const dialog = screen.getByRole("alertdialog")
  expect(client.call).not.toHaveBeenCalled()
  fireEvent.click(within(dialog).getByRole("button", { name: "Cancel" }))
  expect(screen.queryByRole("alertdialog")).toBeNull()
  fireEvent.click(screen.getByRole("button", { name: "Enter" }))
  fireEvent.keyDown(screen.getByRole("alertdialog"), { key: "Escape" })
  expect(screen.queryByRole("alertdialog")).toBeNull()
  expect(client.call).not.toHaveBeenCalled()
})

it("keeps the typed text and does not resend it when the socket drops and comes back", async () => {
  client.call.mockRejectedValue(new RpcError("outcome-unknown"))
  renderPanel()
  fireEvent.change(screen.getByRole("textbox"), { target: { value: "deploy" } })
  fireEvent.click(screen.getByRole("button", { name: "Send to agent" }))
  await waitFor(() => expect(useCompanion.getState().notice?.tone).toBe("warning"))
  useCompanion.setState({ socket: "waiting" })
  useCompanion.setState({ socket: "ready", epoch: useCompanion.getState().epoch + 1 })
  await new Promise((resolve) => setTimeout(resolve, 50))
  expect(client.call).toHaveBeenCalledTimes(1)
  // The user decides: the text is still in the box, untouched.
  expect(screen.getByRole("textbox")).toHaveValue("deploy")
})

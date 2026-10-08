import { cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react"
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"
import i18n from "@/lib/i18n"
import { useMachinesInteractiveStore } from "@/state/machinesInteractiveStore"
import { useMachinesStore } from "@/state/machinesStore"
import { machine, snapshot, supportedCaps } from "@/test/machinesFixtures"
import { MachinesPanel } from "./MachinesPanel"

const ipc = vi.hoisted(() => ({
  caps: vi.fn(), list: vi.fn(), status: vi.fn(), rename: vi.fn(), setEnabled: vi.fn(), remove: vi.fn(), windows: false
}))
vi.mock("@/lib/machinesIpc", () => ({
  machinesCapabilities: ipc.caps, machinesList: ipc.list, machinesStatus: ipc.status, machinesAgents: vi.fn(),
  machinesRename: ipc.rename, machinesSetEnabled: ipc.setEnabled, machinesRemove: ipc.remove
}))
vi.mock("@/lib/platform", async original => ({ ...await original<typeof import("@/lib/platform")>(), isWindowsPlatform: () => ipc.windows }))

const lab = machine("lab", { label: "Lab box" })
const onClose = vi.fn()
const row = () => screen.getByText("Lab box").closest("li")!

beforeEach(async () => {
  vi.resetAllMocks()
  ipc.windows = false
  await i18n.changeLanguage("en")
  useMachinesStore.getState().reset()
  useMachinesInteractiveStore.setState({ selection: null })
  ipc.caps.mockResolvedValue(supportedCaps)
  ipc.list.mockResolvedValue([lab])
})
afterEach(cleanup)

async function mount() {
  render(<MachinesPanel onClose={onClose} />)
  await screen.findByText("Lab box")
}

describe("MachinesPanel actions", () => {
  it("renames inline and adopts the returned list", async () => {
    await mount()
    ipc.rename.mockResolvedValue([{ ...lab, label: "Renamed" }])
    fireEvent.click(within(row()).getByRole("button", { name: "Rename" }))
    fireEvent.change(screen.getByRole("textbox", { name: "Machine name" }), { target: { value: " Renamed " } })
    fireEvent.click(screen.getByRole("button", { name: "Save" }))
    await screen.findByText("Renamed")
    expect(ipc.rename).toHaveBeenCalledWith(lab.id, "Renamed")
  })

  it("disables a machine and hides nothing else", async () => {
    await mount()
    ipc.setEnabled.mockResolvedValue([{ ...lab, enabled: false }])
    fireEvent.click(within(row()).getByRole("button", { name: "Disable" }))
    await waitFor(() => expect(within(row()).getByRole("button", { name: "Enable" })).toBeInTheDocument())
    expect(ipc.setEnabled).toHaveBeenCalledWith(lab.id, false)
    expect(within(row()).getByText("Disabled")).toBeInTheDocument()
  })

  it("confirms removal and explains the remote server is untouched", async () => {
    await mount()
    ipc.remove.mockResolvedValue([])
    fireEvent.click(within(row()).getByRole("button", { name: "Remove" }))
    const dialog = await screen.findByRole("alertdialog")
    expect(dialog).toHaveTextContent("remote HERDR server")
    expect(ipc.remove).not.toHaveBeenCalled()
    fireEvent.click(within(dialog).getByRole("button", { name: "Remove" }))
    await waitFor(() => expect(screen.queryByText("Lab box")).not.toBeInTheDocument())
    expect(ipc.remove).toHaveBeenCalledWith(lab.id)
  })

  it("checks status and renders the reported badge", async () => {
    await mount()
    ipc.status.mockResolvedValue({ id: lab.id, label: lab.label, status: "auth-required", error: null })
    fireEvent.click(within(row()).getByRole("button", { name: "Check status" }))
    await waitFor(() => expect(within(row()).getByText("Authentication required")).toBeInTheDocument())
  })

  it("shows reachable once a snapshot exists and an error message when the last refresh failed", async () => {
    await mount()
    useMachinesStore.setState({ snapshotById: { [lab.id]: snapshot(lab.id) } })
    await waitFor(() => expect(within(row()).getByText("Reachable")).toBeInTheDocument())
    useMachinesStore.setState({ errorById: { [lab.id]: "machines-host-key: x" }, staleById: { [lab.id]: true } })
    await waitFor(() => expect(within(row()).getByText(/host key verification failed/i)).toBeInTheDocument())
    expect(within(row()).getByText("Stale")).toBeInTheDocument()
  })

  it("opens the official client for the machine and closes the picker", async () => {
    await mount()
    fireEvent.click(within(row()).getByRole("button", { name: "Open in official client" }))
    expect(useMachinesInteractiveStore.getState().selection).toEqual({ spec: { kind: "client" }, machineLabel: "Lab box" })
    expect(onClose).toHaveBeenCalled()
  })

  it("starts a reconnect terminal", async () => {
    await mount()
    fireEvent.click(within(row()).getByRole("button", { name: "Reconnect" }))
    expect(useMachinesInteractiveStore.getState().selection).toEqual({ spec: { kind: "reconnect", machineId: lab.id }, machineLabel: "Lab box" })
  })

  it("hides Reconnect on Windows and shows the ssh-agent hint", async () => {
    ipc.windows = true
    await mount()
    expect(within(row()).queryByRole("button", { name: "Reconnect" })).not.toBeInTheDocument()
    expect(screen.getByText(/ssh-agent/, { selector: "p.text-xs" })).toBeInTheDocument()
  })
  it("shows a dedicated message (not the client limit) when an interactive dialog is already open", async () => {
    await mount()
    useMachinesInteractiveStore.setState({ selection: { spec: { kind: "client" } } })
    fireEvent.click(within(row()).getByRole("button", { name: "Reconnect" }))
    expect(await screen.findByRole("alert")).toHaveTextContent("already open")
    expect(screen.queryByText(/Too many official clients/)).not.toBeInTheDocument()
    expect(onClose).not.toHaveBeenCalled()
  })

  it("does not let a stale error override a newer successful status check", async () => {
    await mount()
    useMachinesStore.setState({ errorById: { [lab.id]: "machines-auth-required" } })
    await waitFor(() => expect(within(row()).getByText("Authentication required")).toBeInTheDocument())
    ipc.status.mockResolvedValue({ id: lab.id, label: lab.label, status: "reachable", error: null })
    fireEvent.click(within(row()).getByRole("button", { name: "Check status" }))
    await waitFor(() => expect(within(row()).getByText("Reachable")).toBeInTheDocument())
  })

  it("opening the tab requests only a soft refresh, keeping auth/backoff blocks", async () => {
    await mount()
    await waitFor(() => expect(useMachinesStore.getState().refreshNonce).toBe(1))
    expect(useMachinesStore.getState().refreshForce).toBe(false)
  })

  it("says the binary is unavailable (not too old) when there is no active HERDR binary", async () => {
    ipc.caps.mockResolvedValue({ ...supportedCaps, supported: false, version: null, reason: "machines-binary-unavailable" })
    render(<MachinesPanel onClose={onClose} />)
    expect(await screen.findByText("HERDR binary not available")).toBeInTheDocument()
    expect(screen.getByText("The HERDR binary is unavailable.")).toBeInTheDocument()
    expect(screen.queryByText(/too old/)).not.toBeInTheDocument()
  })

  it("keeps the too-old message for an old runtime", async () => {
    ipc.caps.mockResolvedValue({ ...supportedCaps, supported: false, version: "0.9.0", reason: "machines-runtime-too-old" })
    render(<MachinesPanel onClose={onClose} />)
    expect(await screen.findByText(/0\.9\.0\) is too old/)).toBeInTheDocument()
  })

  it("shows the capability probe failure with a retry instead of an empty list", async () => {
    ipc.caps.mockRejectedValueOnce("machines-binary-unavailable")
    render(<MachinesPanel onClose={onClose} />)
    expect(await screen.findByText("Could not check HERDR machines support")).toBeInTheDocument()
    expect(screen.getByText("The HERDR binary is unavailable.")).toBeInTheDocument()
    expect(screen.queryByText("No HERDR machines yet")).not.toBeInTheDocument()
    ipc.caps.mockResolvedValue(supportedCaps)
    fireEvent.click(screen.getByRole("button", { name: "Retry" }))
    await screen.findByText("Lab box").catch(() => undefined)
    await waitFor(() => expect(useMachinesStore.getState().capabilities?.supported).toBe(true))
  })

  it("keeps a row busy until its own action finishes when another row's action ends first", async () => {
    const other = machine("lab2", { label: "Other box" })
    ipc.list.mockResolvedValue([lab, other])
    render(<MachinesPanel onClose={onClose} />)
    await screen.findByText("Other box")
    const rowOf = (label: string) => screen.getByText(label).closest("li")!
    let finishFirst!: (v: unknown) => void
    ipc.status.mockReturnValueOnce(new Promise(resolve => { finishFirst = resolve }))
    ipc.status.mockResolvedValueOnce({ id: other.id, label: other.label, status: "reachable", error: null })
    fireEvent.click(within(rowOf("Lab box")).getByRole("button", { name: "Check status" }))
    fireEvent.click(within(rowOf("Other box")).getByRole("button", { name: "Check status" }))
    await waitFor(() => expect(within(rowOf("Other box")).getByText("Reachable")).toBeInTheDocument())
    expect(within(rowOf("Lab box")).getByRole("button", { name: "Check status" })).toBeDisabled()
    finishFirst({ id: lab.id, label: lab.label, status: "reachable", error: null })
    await waitFor(() => expect(within(rowOf("Lab box")).getByRole("button", { name: "Check status" })).toBeEnabled())
  })

})

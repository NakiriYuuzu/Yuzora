import { act, cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react"
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"
import i18n from "@/lib/i18n"
import type { HerdrTerminalEvent } from "@/lib/herdrTypes"
import { useMachinesInteractiveStore } from "@/state/machinesInteractiveStore"
import { useMachinesStore } from "@/state/machinesStore"
import { machine } from "@/test/machinesFixtures"
import MachineInteractiveDialog from "./MachineInteractiveDialog"

const mocks = vi.hoisted(() => ({
  open: vi.fn(), release: vi.fn(), resize: vi.fn(), input: vi.fn(), graphicsWrite: vi.fn(), list: vi.fn(),
  toastOk: vi.fn(), toastWarn: vi.fn(), toastError: vi.fn(), copyError: undefined as undefined | (() => void)
}))
vi.mock("@/lib/machinesIpc", () => ({
  machinesInteractiveOpen: mocks.open, machinesList: mocks.list, machinesCapabilities: vi.fn(), machinesStatus: vi.fn(),
  machinesAgents: vi.fn(), machinesRename: vi.fn(), machinesSetEnabled: vi.fn(), machinesRemove: vi.fn()
}))
vi.mock("@/lib/herdrIpc", () => ({
  herdrTerminalRelease: mocks.release, herdrTerminalResize: mocks.resize, herdrTerminalInput: mocks.input
}))
vi.mock("sonner", () => ({ toast: { success: mocks.toastOk, warning: mocks.toastWarn, error: mocks.toastError } }))
vi.mock("@tauri-apps/plugin-clipboard-manager", () => ({ writeText: vi.fn() }))
vi.mock("@/terminal/kittyRenderer", () => ({ installKittyRenderer: () => ({ write: mocks.graphicsWrite, dispose: vi.fn() }) }))
vi.mock("@/terminal/terminalClipboard", () => ({
  installTerminalClipboardHandling: (_term: unknown, options: { onCopyError?: () => void }) => {
    mocks.copyError = options.onCopyError
    return { flushPendingPaste: vi.fn(), dispose: vi.fn() }
  }
}))
vi.mock("@xterm/addon-fit", () => ({ FitAddon: class { fit = vi.fn() } }))
vi.mock("@xterm/xterm", () => ({
  Terminal: class {
    cols = 80
    rows = 24
    options = {}
    element?: HTMLElement
    parser = { registerOscHandler: () => ({ dispose: vi.fn() }) }
    open(element: HTMLElement) {
      this.element = document.createElement("div")
      this.element.innerHTML = '<div class="xterm-screen"></div>'
      element.append(this.element)
    }
    loadAddon() {}
    onData() { return { dispose: vi.fn() } }
    focus() {}
    dispose = vi.fn()
  }
}))

let onEvent!: (event: HerdrTerminalEvent) => void
const frame = (text: string): HerdrTerminalEvent => ({ type: "frame", sessionId: "herdr-client-1", seq: 1, full: false, encoding: "ansi", width: 80, height: 24, bytesBase64: btoa(text) })
const nextFrame = () => act(async () => { await new Promise<void>(resolve => requestAnimationFrame(() => resolve())) })

beforeEach(async () => {
  vi.resetAllMocks()
  await i18n.changeLanguage("en")
  mocks.open.mockImplementation(async (_spec, _size, listener) => { onEvent = listener; return { sessionId: "herdr-client-1" } })
  for (const fn of [mocks.release, mocks.resize, mocks.input, mocks.graphicsWrite]) fn.mockResolvedValue(undefined)
  useMachinesStore.getState().reset()
  useMachinesInteractiveStore.setState({ selection: null })
})
afterEach(cleanup)

describe("MachineInteractiveDialog", () => {
  it("opens the add spec with the terminal size and shows the cancel hint", async () => {
    const spec = { kind: "add", target: "me@box" } as const
    render(<MachineInteractiveDialog selection={{ spec }} />)
    await nextFrame()
    await waitFor(() => expect(mocks.open).toHaveBeenCalledTimes(1))
    expect(mocks.open.mock.calls[0][0]).toEqual(spec)
    expect(mocks.open.mock.calls[0][1]).toMatchObject({ cols: 80, rows: 24 })
    await waitFor(() => expect(mocks.resize).toHaveBeenCalledWith("herdr-client-1", 80, 24))
    expect(screen.getByText(/Esc or Ctrl\+C cancels/)).toBeInTheDocument()
  })

  it("renders frames and shows the ended state with a Close button on closed", async () => {
    render(<MachineInteractiveDialog selection={{ spec: { kind: "client" }, machineLabel: "Lab box" }} />)
    await nextFrame()
    await waitFor(() => expect(mocks.resize).toHaveBeenCalled())
    expect(screen.getByRole("dialog", { name: "Select Lab box in the sidebar" })).toBeInTheDocument()
    await act(async () => { onEvent(frame("hello")) })
    expect(mocks.graphicsWrite).toHaveBeenCalledTimes(1)
    await act(async () => { onEvent({ type: "closed", sessionId: "herdr-client-1" }) })
    expect(screen.getByText("The session has ended.")).toBeInTheDocument()
    expect(mocks.release).toHaveBeenCalledWith("herdr-client-1")
    fireEvent.click(screen.getAllByRole("button", { name: "Close" }).at(-1)!)
    expect(useMachinesInteractiveStore.getState().selection).toBeNull()
  })

  it("still renders frames queued before the official command exits", async () => {
    render(<MachineInteractiveDialog selection={{ spec: { kind: "client" }, machineLabel: "Lab box" }} />)
    await nextFrame()
    await waitFor(() => expect(mocks.resize).toHaveBeenCalled())
    let releaseFirst!: () => void
    mocks.graphicsWrite.mockImplementationOnce(() => new Promise<void>(resolve => { releaseFirst = resolve }))
    await act(async () => { onEvent(frame("first")); onEvent(frame("last line")) })
    await act(async () => { onEvent({ type: "closed", sessionId: "herdr-client-1" }) })
    expect(mocks.release).not.toHaveBeenCalled()
    await act(async () => { releaseFirst() })
    await waitFor(() => expect(mocks.graphicsWrite).toHaveBeenCalledTimes(2))
    await waitFor(() => expect(mocks.release).toHaveBeenCalledWith("herdr-client-1"))
  })

  it("reports a clipboard failure without ending the official command", async () => {
    render(<MachineInteractiveDialog selection={{ spec: { kind: "client" }, machineLabel: "Lab box" }} />)
    await nextFrame()
    await waitFor(() => expect(mocks.resize).toHaveBeenCalled())
    act(() => { mocks.copyError?.() })
    expect(mocks.toastError).toHaveBeenCalled()
    expect(mocks.release).not.toHaveBeenCalled()
    expect(screen.queryByText("The session has ended.")).not.toBeInTheDocument()
  })

  it("forces a machines refresh when the official client closes", async () => {
    useMachinesStore.setState({ refreshNonce: 0, refreshForce: false })
    useMachinesInteractiveStore.setState({ selection: { spec: { kind: "client" }, machineLabel: "Lab box" } })
    render(<MachineInteractiveDialog selection={{ spec: { kind: "client" }, machineLabel: "Lab box" }} />)
    await nextFrame()
    await waitFor(() => expect(mocks.resize).toHaveBeenCalled())
    await act(async () => { onEvent({ type: "closed", sessionId: "herdr-client-1" }) })
    fireEvent.click(screen.getAllByRole("button", { name: "Close" }).at(-1)!)
    expect(useMachinesStore.getState().refreshNonce).toBe(1)
    expect(useMachinesStore.getState().refreshForce).toBe(true)
  })

  it("cannot confirm an add when no machine list had loaded before it", async () => {
    mocks.list.mockResolvedValue([machine("old", { target: "me@box" })])
    useMachinesInteractiveStore.setState({ selection: { spec: { kind: "add", target: "me@box" } } })
    render(<MachineInteractiveDialog selection={{ spec: { kind: "add", target: "me@box" } }} />)
    await nextFrame()
    await waitFor(() => expect(mocks.resize).toHaveBeenCalled())
    await act(async () => { onEvent({ type: "closed", sessionId: "herdr-client-1" }) })
    fireEvent.click(screen.getAllByRole("button", { name: "Close" }).at(-1)!)
    await waitFor(() => expect(mocks.toastWarn).toHaveBeenCalled())
    expect(mocks.toastOk).not.toHaveBeenCalled()
  })

  it("treats a command that ended before the open response as completed, not failed", async () => {
    mocks.open.mockImplementation(async (_spec, _size, listener) => {
      onEvent = listener
      listener({ type: "closed", sessionId: "herdr-client-1" })
      return { sessionId: "herdr-client-1" }
    })
    render(<MachineInteractiveDialog selection={{ spec: { kind: "client" }, machineLabel: "Lab box" }} />)
    await nextFrame()
    await waitFor(() => expect(mocks.release).toHaveBeenCalledWith("herdr-client-1"))
    expect(screen.getByText("The session has ended.")).toBeInTheDocument()
    expect(mocks.resize).not.toHaveBeenCalled()
    expect(screen.queryByRole("alert")).not.toBeInTheDocument()
  })

  it("reports a saved machine after an add finishes", async () => {
    const added = machine("new", { target: "me@box" })
    mocks.list.mockResolvedValue([added])
    // A loaded (empty) list is the baseline that makes the new id provably new.
    useMachinesStore.setState({ machines: [], listLoaded: true })
    useMachinesInteractiveStore.setState({ selection: { spec: { kind: "add", target: "me@box" } } })
    render(<MachineInteractiveDialog selection={{ spec: { kind: "add", target: "me@box" } }} />)
    await nextFrame()
    await waitFor(() => expect(mocks.resize).toHaveBeenCalled())
    await act(async () => { onEvent({ type: "closed", sessionId: "herdr-client-1" }) })
    fireEvent.click(screen.getAllByRole("button", { name: "Close" }).at(-1)!)
    await waitFor(() => expect(mocks.toastOk).toHaveBeenCalledWith("Machine added: me@box"))
    expect(mocks.toastWarn).not.toHaveBeenCalled()
  })

  it("warns when the machine was not saved", async () => {
    mocks.list.mockResolvedValue([])
    useMachinesStore.setState({ machines: [], listLoaded: true })
    useMachinesInteractiveStore.setState({ selection: { spec: { kind: "add", target: "me@box" } } })
    render(<MachineInteractiveDialog selection={{ spec: { kind: "add", target: "me@box" } }} />)
    await nextFrame()
    await waitFor(() => expect(mocks.resize).toHaveBeenCalled())
    await act(async () => { onEvent({ type: "closed", sessionId: "herdr-client-1" }) })
    fireEvent.click(screen.getAllByRole("button", { name: "Close" }).at(-1)!)
    await waitFor(() => expect(mocks.toastWarn).toHaveBeenCalledWith("Machine was not saved"))
    expect(mocks.toastOk).not.toHaveBeenCalled()
  })

  it("localises the native client limit error", async () => {
    mocks.open.mockRejectedValue("native-client-limit")
    render(<MachineInteractiveDialog selection={{ spec: { kind: "client" } }} />)
    await nextFrame()
    expect(await screen.findByRole("alert")).toHaveTextContent("Too many official clients are open")
  })
  it("does not claim the machine was not saved when the list refresh fails", async () => {
    mocks.list.mockRejectedValue("machines-timeout")
    useMachinesInteractiveStore.setState({ selection: { spec: { kind: "add", target: "me@box" } } })
    render(<MachineInteractiveDialog selection={{ spec: { kind: "add", target: "me@box" } }} />)
    await nextFrame()
    await waitFor(() => expect(mocks.resize).toHaveBeenCalled())
    await act(async () => { onEvent({ type: "closed", sessionId: "herdr-client-1" }) })
    fireEvent.click(screen.getAllByRole("button", { name: "Close" }).at(-1)!)
    await waitFor(() => expect(mocks.toastWarn).toHaveBeenCalledWith("Could not confirm whether the machine was saved. Refresh the list."))
    expect(mocks.toastWarn).not.toHaveBeenCalledWith("Machine was not saved")
  })
})

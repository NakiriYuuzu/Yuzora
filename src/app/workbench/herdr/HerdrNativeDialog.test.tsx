import { act, cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react"
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"
import i18n from "@/lib/i18n"
import { useHerdrNativeStore, type HerdrNativeSelection } from "@/state/herdrNativeStore"
import HerdrNativeDialog from "./HerdrNativeDialog"

const mocks = vi.hoisted(() => ({
  invoke: vi.fn(), feature: vi.fn(), release: vi.fn(), resize: vi.fn(), input: vi.fn(), paneFocus: vi.fn(),
  bootstrap: vi.fn(), terminalOpen: vi.fn(), terminalDispose: vi.fn(), graphicsDispose: vi.fn(), fit: vi.fn(), onData: vi.fn()
}))

vi.mock("@/lib/herdrProvider", () => ({ invokeHerdr: mocks.invoke }))
vi.mock("@/lib/herdrFeatures", () => ({ herdrFeature: mocks.feature }))
vi.mock("@/lib/herdrIpc", () => ({
  herdrTerminalRelease: mocks.release, herdrTerminalResize: mocks.resize,
  herdrTerminalInput: mocks.input, herdrPaneFocus: mocks.paneFocus
}))
vi.mock("@/state/herdrStore", () => ({ useHerdrStore: { getState: () => ({ bootstrap: mocks.bootstrap }) } }))
vi.mock("@/terminal/kittyRenderer", () => ({
  installKittyRenderer: () => ({ write: vi.fn().mockResolvedValue(undefined), dispose: mocks.graphicsDispose })
}))
vi.mock("@/terminal/terminalClipboard", () => ({
  installTerminalClipboardHandling: () => ({ flushPendingPaste: vi.fn(), dispose: vi.fn() })
}))
vi.mock("@xterm/addon-fit", () => ({ FitAddon: class { fit = mocks.fit } }))
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
      mocks.terminalOpen(element)
    }
    loadAddon() {}
    onData(listener: (text: string) => void) { mocks.onData(listener); return { dispose: vi.fn() } }
    focus() {}
    dispose = mocks.terminalDispose
  }
}))

// Keep the real shadcn/Radix portal: the original ref-only effect ran before
// this delayed content mounted and never opened the native client.
function NativeDialogHost() {
  const selection = useHerdrNativeStore(state => state.selection)
  return selection ? <HerdrNativeDialog selection={selection} /> : null
}

async function flushAnimationFrame() {
  await act(async () => { await new Promise<void>(resolve => requestAnimationFrame(() => resolve())) })
}

beforeEach(() => {
  vi.clearAllMocks()
  mocks.invoke.mockResolvedValue({ sessionId: "native-client-1" })
  mocks.feature.mockResolvedValue({})
  for (const fn of [mocks.release, mocks.resize, mocks.input, mocks.paneFocus, mocks.bootstrap]) fn.mockResolvedValue(undefined)
  useHerdrNativeStore.setState({ selection: null })
})

afterEach(() => {
  cleanup()
  useHerdrNativeStore.setState({ selection: null })
})

describe("HERDR native client dialog lifecycle", () => {
  it("serializes pre-open input before input typed while that first write is pending", async () => {
    let open!: (value: { sessionId: string }) => void
    let finishInput!: () => void
    mocks.invoke.mockReturnValueOnce(new Promise(resolve => { open = resolve }))
    mocks.input.mockReturnValueOnce(new Promise<void>(resolve => { finishInput = resolve }))
    render(<HerdrNativeDialog selection={{ sessionName: "e2e-session" }} />)
    await waitFor(() => expect(mocks.invoke).toHaveBeenCalledTimes(1))
    const type = mocks.onData.mock.calls[0][0] as (text: string) => void
    act(() => { type("first"); type(" input") })
    await act(async () => { open({ sessionId: "native-client-1" }) })
    expect(mocks.input).toHaveBeenCalledExactlyOnceWith("native-client-1", "first input")

    await act(async () => { type(" second"); type("\r") })
    expect(mocks.input).toHaveBeenCalledTimes(1)
    await act(async () => { finishInput() })
    expect(mocks.input.mock.calls).toEqual([
      ["native-client-1", "first input"], ["native-client-1", " second"], ["native-client-1", "\r"]
    ])
  })

  it("opens after the real portal mounts and releases the client when its close button is used", async () => {
    useHerdrNativeStore.getState().open({ sessionName: "e2e-session", paneId: "pane-1" })
    const { container } = render(<NativeDialogHost />)
    const dialog = await screen.findByRole("dialog")
    expect(container).not.toContainElement(dialog)
    await waitFor(() => expect(mocks.invoke).toHaveBeenCalledTimes(1))
    expect(mocks.invoke).toHaveBeenCalledWith("herdr_client_open", expect.objectContaining({
      sessionName: "e2e-session", size: expect.objectContaining({ cols: 80, rows: 24 }), onEvent: expect.any(Function)
    }))
    expect(mocks.terminalOpen).toHaveBeenCalledWith(expect.any(HTMLDivElement))
    expect(dialog).toContainElement(mocks.terminalOpen.mock.calls[0][0])
    await waitFor(() => expect(mocks.paneFocus).toHaveBeenCalledWith({ sessionName: "e2e-session", paneId: "pane-1" }))
    expect(screen.queryByRole("status")).not.toBeInTheDocument()

    fireEvent.click(screen.getByRole("button", { name: "Close" }))
    await waitFor(() => expect(screen.queryByRole("dialog")).not.toBeInTheDocument())
    expect(mocks.release).toHaveBeenCalledExactlyOnceWith("native-client-1")
    expect(mocks.terminalDispose).toHaveBeenCalledTimes(1)
    expect(mocks.graphicsDispose).toHaveBeenCalledTimes(1)
    expect(mocks.bootstrap).toHaveBeenCalledExactlyOnceWith("e2e-session")
  })

  it("updates translated content without reopening the client or replaying a plugin action", async () => {
    const selection: HerdrNativeSelection = {
      sessionName: "e2e-session",
      request: { method: "plugin.action.invoke", params: { plugin_id: "fixture.demo", action_id: "inspect" } }
    }
    useHerdrNativeStore.getState().open(selection)
    render(<NativeDialogHost />)
    await waitFor(() => expect(mocks.feature).toHaveBeenCalledExactlyOnceWith("e2e-session", selection.request))
    const englishDescription = i18n.t("herdrTools:nativeDescription")

    await act(async () => { await i18n.changeLanguage("zh-TW") })
    const translatedDescription = i18n.t("herdrTools:nativeDescription")
    expect(translatedDescription).not.toBe(englishDescription)
    expect(screen.getByText(translatedDescription)).toBeInTheDocument()
    await flushAnimationFrame()
    await flushAnimationFrame()

    expect(mocks.invoke).toHaveBeenCalledTimes(1)
    expect(mocks.feature).toHaveBeenCalledTimes(1)
    expect(mocks.release).not.toHaveBeenCalled()
    expect(mocks.terminalDispose).not.toHaveBeenCalled()
  })

  it("does not replay a plugin action when the client remounts for the same selection", async () => {
    const selection: HerdrNativeSelection = {
      sessionName: "e2e-session",
      request: { method: "plugin.action.invoke", params: { plugin_id: "fixture.demo", action_id: "inspect" } }
    }
    const first = render(<HerdrNativeDialog selection={selection} />)
    await waitFor(() => expect(mocks.feature).toHaveBeenCalledTimes(1))
    first.unmount()
    render(<HerdrNativeDialog selection={selection} />)
    await waitFor(() => expect(mocks.invoke).toHaveBeenCalledTimes(2))
    await flushAnimationFrame()
    await flushAnimationFrame()
    expect(mocks.feature).toHaveBeenCalledTimes(1)
  })

  it("focuses the source before opening a plugin tab, then focuses its returned pane", async () => {
    const selection: HerdrNativeSelection = {
      sessionName: "e2e-session", paneId: "source-pane",
      request: { method: "plugin.pane.open", params: { plugin_id: "fixture.demo", entrypoint: "inspect", placement: "tab", workspace_id: "workspace-1", focus: true } }
    }
    let complete!: (result: unknown) => void
    mocks.feature.mockImplementation(() => new Promise(resolve => { complete = resolve }))
    useHerdrNativeStore.getState().open(selection)
    render(<NativeDialogHost />)
    await waitFor(() => expect(mocks.feature).toHaveBeenCalledExactlyOnceWith("e2e-session", selection.request))
    expect(mocks.paneFocus).toHaveBeenCalledExactlyOnceWith({ sessionName: "e2e-session", paneId: "source-pane" })
    expect(mocks.invoke.mock.invocationCallOrder[0]).toBeLessThan(mocks.paneFocus.mock.invocationCallOrder[0])
    expect(mocks.paneFocus.mock.invocationCallOrder[0]).toBeLessThan(mocks.feature.mock.invocationCallOrder[0])

    await act(async () => { complete({ plugin_pane: { pane: { pane_id: "new-plugin-pane" } } }) })
    await waitFor(() => expect(mocks.paneFocus).toHaveBeenCalledTimes(2))
    expect(mocks.paneFocus).toHaveBeenNthCalledWith(2, { sessionName: "e2e-session", paneId: "new-plugin-pane" })
    expect(mocks.feature.mock.invocationCallOrder[0]).toBeLessThan(mocks.paneFocus.mock.invocationCallOrder[1])
    expect(screen.queryByRole("alert")).not.toBeInTheDocument()
  })

  it("keeps the source focused when a plugin popup has no ordinary pane in its result", async () => {
    const selection: HerdrNativeSelection = {
      sessionName: "e2e-session", paneId: "source-pane",
      request: { method: "plugin.pane.open", params: { plugin_id: "fixture.demo", entrypoint: "inspect", placement: "popup", focus: true } }
    }
    mocks.feature.mockResolvedValue({ plugin_pane: { placement: "popup" } })
    useHerdrNativeStore.getState().open(selection)
    render(<NativeDialogHost />)
    await waitFor(() => expect(mocks.feature).toHaveBeenCalledExactlyOnceWith("e2e-session", selection.request))
    await flushAnimationFrame()

    expect(mocks.paneFocus).toHaveBeenCalledExactlyOnceWith({ sessionName: "e2e-session", paneId: "source-pane" })
    expect(screen.queryByRole("alert")).not.toBeInTheDocument()
    expect(mocks.release).not.toHaveBeenCalled()
  })
})

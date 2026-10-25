import { act, cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react"
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"
import i18n from "@/lib/i18n"
import type { HerdrTerminalEvent } from "@/lib/herdrTypes"
import { useHerdrNativeStore, type HerdrNativeSelection } from "@/state/herdrNativeStore"
import HerdrNativeDialog from "./HerdrNativeDialog"

const mocks = vi.hoisted(() => ({
  invoke: vi.fn(), feature: vi.fn(), release: vi.fn(), resize: vi.fn(), input: vi.fn(), paneFocus: vi.fn(),
  bootstrap: vi.fn(), terminalOpen: vi.fn(), terminalDispose: vi.fn(), graphicsDispose: vi.fn(), fit: vi.fn(), onData: vi.fn(),
  graphicsWrite: vi.fn(), writeText: vi.fn(), oscHandler: vi.fn(), clipboardOptions: vi.fn()
}))

vi.mock("@/lib/herdrProvider", () => ({ invokeHerdr: mocks.invoke }))
vi.mock("@/lib/herdrFeatures", () => ({ herdrFeature: mocks.feature }))
vi.mock("@/lib/herdrIpc", () => ({
  herdrTerminalRelease: mocks.release, herdrTerminalResize: mocks.resize,
  herdrTerminalInput: mocks.input, herdrPaneFocus: mocks.paneFocus
}))
vi.mock("@/state/herdrStore", () => ({ useHerdrStore: { getState: () => ({ bootstrap: mocks.bootstrap }) } }))
vi.mock("@tauri-apps/plugin-clipboard-manager", () => ({ writeText: mocks.writeText }))
vi.mock("@/terminal/kittyRenderer", () => ({
  installKittyRenderer: () => ({ write: mocks.graphicsWrite, dispose: mocks.graphicsDispose })
}))
vi.mock("@/terminal/terminalClipboard", () => ({
  installTerminalClipboardHandling: (_term: unknown, options: unknown) => { mocks.clipboardOptions(options); return { flushPendingPaste: vi.fn(), dispose: vi.fn() } }
}))
vi.mock("@xterm/addon-fit", () => ({ FitAddon: class { fit = mocks.fit } }))
vi.mock("@xterm/xterm", () => ({
  Terminal: class {
    cols = 80
    rows = 24
    options = {}
    element?: HTMLElement
    parser = { registerOscHandler: (code: number, handler: (data: string) => boolean) => { mocks.oscHandler(code, handler); return { dispose: vi.fn() } } }
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
  for (const fn of [mocks.release, mocks.resize, mocks.input, mocks.paneFocus, mocks.bootstrap, mocks.graphicsWrite, mocks.writeText]) fn.mockResolvedValue(undefined)
  useHerdrNativeStore.setState({ selection: null })
})

afterEach(() => {
  cleanup()
  useHerdrNativeStore.setState({ selection: null })
})

describe("HERDR native client dialog lifecycle", () => {
  it.each(["resize", "source", "feature", "returned"] as const)("waits for %s initialization before admitting user paste while preserving protocol replies", async phase => {
    let finish!: () => void
    const pending = new Promise<void>(resolve => { finish = resolve })
    const result = { plugin_pane: { pane: { pane_id: "result-pane" } } }
    mocks.feature.mockResolvedValue(result)
    if (phase === "resize") mocks.resize.mockReturnValueOnce(pending)
    if (phase === "source") mocks.paneFocus.mockReturnValueOnce(pending)
    if (phase === "feature") mocks.feature.mockImplementationOnce(async () => { await pending; return result })
    if (phase === "returned") mocks.paneFocus.mockImplementation(({ paneId }) => paneId === "result-pane" ? pending : Promise.resolve())
    render(<HerdrNativeDialog selection={{ sessionName: "fixture", paneId: "source-pane", request: {
      method: "plugin.pane.open", params: { plugin_id: "fixture", entrypoint: "inspect", placement: "tab", focus: true }
    } }} />)
    await waitFor(() => {
      if (phase === "resize") expect(mocks.resize).toHaveBeenCalledTimes(1)
      if (phase === "source") expect(mocks.paneFocus).toHaveBeenCalledTimes(1)
      if (phase === "feature") expect(mocks.feature).toHaveBeenCalledTimes(1)
      if (phase === "returned") expect(mocks.paneFocus).toHaveBeenCalledTimes(2)
    })
    const clipboard = mocks.clipboardOptions.mock.calls[0][0] as { canPaste: () => boolean }
    expect(clipboard.canPaste()).toBe(false)
    expect(screen.getByRole("status")).toBeInTheDocument()
    await act(async () => { mocks.onData.mock.calls[0][0]("\x1b[0n") })
    expect(mocks.input).toHaveBeenCalledExactlyOnceWith("native-client-1", "\x1b[0n")
    await act(async () => { finish() })
    await waitFor(() => expect(screen.queryByRole("status")).not.toBeInTheDocument())
    expect(clipboard.canPaste()).toBe(true)
    expect(mocks.terminalOpen.mock.calls[0][0]).not.toHaveAttribute("inert")
  })

  it("waits for the original pending plugin request after remount without replaying it", async () => {
    let finish!: (result: unknown) => void
    mocks.feature.mockImplementationOnce(() => new Promise(resolve => { finish = resolve }))
    const selection: HerdrNativeSelection = { sessionName: "fixture", paneId: "source-pane", request: {
      method: "plugin.pane.open", params: { plugin_id: "fixture", entrypoint: "inspect", placement: "tab", focus: true }
    } }
    const first = render(<HerdrNativeDialog selection={selection} />)
    await waitFor(() => expect(mocks.feature).toHaveBeenCalledTimes(1))
    first.unmount()
    render(<HerdrNativeDialog selection={selection} />)
    await waitFor(() => expect(mocks.paneFocus).toHaveBeenCalledTimes(2))
    const clipboard = mocks.clipboardOptions.mock.calls.at(-1)![0] as { canPaste: () => boolean }
    expect(clipboard.canPaste()).toBe(false)
    expect(screen.getByRole("status")).toBeInTheDocument()
    expect(mocks.feature).toHaveBeenCalledTimes(1)
    await act(async () => { finish({ plugin_pane: { pane: { pane_id: "result-pane" } } }) })
    await waitFor(() => expect(clipboard.canPaste()).toBe(true))
    expect(mocks.paneFocus.mock.calls.filter(([request]) => request.paneId === "result-pane")).toHaveLength(1)
    expect(mocks.feature).toHaveBeenCalledTimes(1)
  })

  function frame(bytesBase64: string): HerdrTerminalEvent {
    return { type: "frame", sessionId: "native-client-1", seq: 1, full: false, encoding: "ansi", width: 80, height: 24, bytesBase64 }
  }

  async function openOutputDialog() {
    const dialog = render(<HerdrNativeDialog selection={{ sessionName: "e2e-session" }} />)
    await waitFor(() => expect(mocks.resize).toHaveBeenCalledTimes(1))
    return {
      ...dialog,
      onEvent: mocks.invoke.mock.calls[0][1].onEvent as (event: HerdrTerminalEvent) => void,
      onOsc: mocks.oscHandler.mock.calls[0][1] as (data: string) => boolean
    }
  }

  it.each([0, 64, 16384])("preserves every byte in a %i-byte native frame", async size => {
    const bytes = Uint8Array.from({ length: size }, (_, index) => (index * 37 + 11) & 255)
    const { onEvent } = await openOutputDialog()
    await act(async () => { onEvent(frame(btoa(String.fromCharCode(...bytes)))) })
    expect(mocks.graphicsWrite).toHaveBeenCalledExactlyOnceWith(bytes)
  })

  it("preserves invalid-frame rejection", async () => {
    const { onEvent } = await openOutputDialog()
    expect(() => onEvent(frame("%%%"))).toThrow()
    expect(mocks.graphicsWrite).not.toHaveBeenCalled()
  })

  it("keeps decoded output ordered while the first renderer write is pending", async () => {
    let finish!: () => void
    mocks.graphicsWrite.mockReturnValueOnce(new Promise<void>(resolve => { finish = resolve }))
    const { onEvent } = await openOutputDialog()
    await act(async () => { onEvent(frame(btoa("first"))) })
    await act(async () => { onEvent(frame(btoa("second"))) })
    expect(mocks.graphicsWrite).toHaveBeenCalledTimes(1)
    await act(async () => { finish() })
    expect(mocks.graphicsWrite.mock.calls.map(([bytes]) => new TextDecoder().decode(bytes))).toEqual(["first", "second"])
  })

  it("renders a final frame even when EOF arrives in the same event turn", async () => {
    const { onEvent } = await openOutputDialog()
    await act(async () => {
      onEvent(frame(btoa("final diagnostic")))
      onEvent({ type: "closed", sessionId: "native-client-1", reason: "EOF" })
    })
    expect(mocks.graphicsWrite).toHaveBeenCalledExactlyOnceWith(Uint8Array.from(new TextEncoder().encode("final diagnostic")))
    expect(screen.getByRole("alert")).toHaveTextContent("EOF")
    expect(mocks.release).toHaveBeenCalledExactlyOnceWith("native-client-1")
  })

  it("drains accepted frames on EOF while rejecting new frames and input", async () => {
    let finish!: () => void
    mocks.graphicsWrite.mockReturnValueOnce(new Promise<void>(resolve => { finish = resolve }))
    const { onEvent } = await openOutputDialog()
    await act(async () => { onEvent(frame(btoa("first"))) })
    act(() => {
      onEvent(frame(btoa("final diagnostic")))
      onEvent({ type: "closed", sessionId: "native-client-1", reason: "EOF" })
      onEvent(frame(btoa("late ignored")))
      mocks.onData.mock.calls[0][0]("late input")
    })
    expect(mocks.input).not.toHaveBeenCalled()
    await act(async () => { finish() })
    expect(mocks.graphicsWrite.mock.calls.map(([bytes]) => new TextDecoder().decode(bytes))).toEqual(["first", "final diagnostic"])
  })

  it("aborts an EOF drain when the dialog is unmounted", async () => {
    let finish!: () => void
    mocks.graphicsWrite.mockReturnValueOnce(new Promise<void>(resolve => { finish = resolve }))
    const { onEvent, unmount } = await openOutputDialog()
    await act(async () => { onEvent(frame(btoa("first"))) })
    act(() => { onEvent(frame(btoa("queued"))); onEvent({ type: "closed", sessionId: "native-client-1" }) })
    unmount()
    await act(async () => { finish() })
    expect(mocks.graphicsWrite).toHaveBeenCalledTimes(1)
    expect(mocks.graphicsDispose).toHaveBeenCalledTimes(1)
  })

  it("aborts remaining EOF output if the renderer rejects", async () => {
    let reject!: (reason: Error) => void
    mocks.graphicsWrite.mockReturnValueOnce(new Promise<void>((_resolve, no) => { reject = no }))
    const { onEvent } = await openOutputDialog()
    await act(async () => { onEvent(frame(btoa("first"))) })
    act(() => { onEvent(frame(btoa("queued"))); onEvent({ type: "closed", sessionId: "native-client-1", reason: "EOF" }) })
    await act(async () => { reject(new Error("renderer failed")) })
    expect(mocks.graphicsWrite).toHaveBeenCalledTimes(1)
    expect(screen.getByRole("alert")).toHaveTextContent("renderer failed")
  })

  it("drains EOF received before the open reply without activating the client", async () => {
    let open!: (value: { sessionId: string }) => void
    mocks.invoke.mockReturnValueOnce(new Promise(resolve => { open = resolve }))
    render(<HerdrNativeDialog selection={{ sessionName: "e2e-session", request: { method: "plugin.action.invoke", params: { plugin_id: "fixture", action_id: "inspect" } } }} />)
    await waitFor(() => expect(mocks.invoke).toHaveBeenCalledTimes(1))
    const onEvent = mocks.invoke.mock.calls[0][1].onEvent
    await act(async () => {
      onEvent(frame(btoa("open failed detail")))
      onEvent({ type: "closed", sessionId: "native-client-1", reason: "EOF" })
      open({ sessionId: "native-client-1" })
    })
    expect(mocks.graphicsWrite).toHaveBeenCalledExactlyOnceWith(Uint8Array.from(new TextEncoder().encode("open failed detail")))
    expect(mocks.resize).not.toHaveBeenCalled()
    expect(mocks.feature).not.toHaveBeenCalled()
    expect(mocks.release).toHaveBeenCalledExactlyOnceWith("native-client-1")
  })

  it("does not activate or dispatch a feature after EOF during initial resize", async () => {
    let finish!: () => void
    mocks.resize.mockReturnValueOnce(new Promise<void>(resolve => { finish = resolve }))
    render(<HerdrNativeDialog selection={{ sessionName: "e2e-session", paneId: "source", request: { method: "plugin.action.invoke", params: { plugin_id: "fixture", action_id: "inspect" } } }} />)
    await waitFor(() => expect(mocks.resize).toHaveBeenCalledTimes(1))
    const onEvent = mocks.invoke.mock.calls[0][1].onEvent
    await act(async () => { onEvent({ type: "closed", sessionId: "native-client-1", reason: "EOF" }); finish() })
    expect(mocks.paneFocus).not.toHaveBeenCalled()
    expect(mocks.feature).not.toHaveBeenCalled()
    expect(screen.getByRole("alert")).toHaveTextContent("EOF")
  })

  it.each(["input", "resize", "open-release", "feature", "clipboard"] as const)(
    "does not abort accepted EOF output when a late %s operation fails", async kind => {
      let finishWrite!: () => void, rejectControl!: (cause: Error) => void
      const control = new Promise<void>((_resolve, reject) => { rejectControl = reject })
      let open!: (value: { sessionId: string }) => void
      mocks.graphicsWrite.mockReturnValueOnce(new Promise<void>(resolve => { finishWrite = resolve }))
      if (kind === "input") mocks.input.mockReturnValueOnce(control)
      if (kind === "resize") mocks.resize.mockReturnValueOnce(control)
      if (kind === "feature") mocks.feature.mockReturnValueOnce(control)
      if (kind === "open-release") {
        mocks.invoke.mockReturnValueOnce(new Promise(resolve => { open = resolve }))
        mocks.release.mockReturnValueOnce(control)
      }
      render(<HerdrNativeDialog selection={{ sessionName: "e2e-session", request: kind === "feature" ? { method: "plugin.action.invoke", params: { plugin_id: "fixture", action_id: "inspect" } } : undefined }} />)
      await waitFor(() => expect(mocks.invoke).toHaveBeenCalledTimes(1))
      if (kind !== "open-release") await waitFor(() => expect(mocks.resize).toHaveBeenCalledTimes(1))
      if (kind === "feature") await waitFor(() => expect(mocks.feature).toHaveBeenCalledTimes(1))
      const onEvent = mocks.invoke.mock.calls[0][1].onEvent
      if (kind === "input") {
        await act(async () => mocks.onData.mock.calls[0][0]("in flight"))
        expect(mocks.input).toHaveBeenCalledTimes(1)
      }
      await act(async () => onEvent(frame(btoa("first"))))
      act(() => { onEvent(frame(btoa("final"))); onEvent({ type: "closed", sessionId: "native-client-1", reason: "EOF" }) })
      if (kind === "open-release") await act(async () => open({ sessionId: "native-client-1" }))
      if (kind === "clipboard") {
        act(() => mocks.clipboardOptions.mock.calls[0][0].onCopyError(new Error("copy failed")))
        // This branch does not attach control to a production promise.
        void control.catch(() => {})
      }
      await act(async () => rejectControl(new Error("herdr-client-closed")))
      await act(async () => finishWrite())
      expect(mocks.graphicsWrite.mock.calls.map(([bytes]) => new TextDecoder().decode(bytes))).toEqual(["first", "final"])
      expect(screen.getByRole("alert")).toHaveTextContent(kind === "clipboard" ? "copy failed" : "EOF")
    }
  )

  it("retires queued output and ignores malformed late frames after disposal", async () => {
    let finish!: () => void
    mocks.graphicsWrite.mockReturnValueOnce(new Promise<void>(resolve => { finish = resolve }))
    const { onEvent, unmount } = await openOutputDialog()
    await act(async () => { onEvent(frame(btoa("first"))) })
    act(() => { onEvent(frame(btoa("queued"))) })
    unmount()
    await act(async () => { finish() })
    expect(() => onEvent(frame("%%%"))).not.toThrow()
    expect(mocks.graphicsWrite).toHaveBeenCalledTimes(1)
    expect(mocks.graphicsDispose).toHaveBeenCalledTimes(1)
    expect(mocks.release).toHaveBeenCalledExactlyOnceWith("native-client-1")
  })

  it.each([512, 513])("preserves the 8 MiB queue limit for %i native-sized frames", async count => {
    const { onEvent } = await openOutputDialog()
    const event = frame(btoa("x".repeat(16384)))
    await act(async () => { for (let index = 0; index < count; index++) onEvent(event) })
    if (count === 512) {
      expect(mocks.graphicsWrite).toHaveBeenCalledTimes(512)
      expect(mocks.release).not.toHaveBeenCalled()
      expect(screen.queryByRole("alert")).not.toBeInTheDocument()
    } else {
      expect(mocks.graphicsWrite).not.toHaveBeenCalled()
      expect(mocks.release).toHaveBeenCalledExactlyOnceWith("native-client-1")
      expect(screen.getByRole("alert")).toHaveTextContent(i18n.t("herdrTools:nativeOutputLimit"))
    }
  })

  it("decodes UTF-8 OSC52 text and accepts base64 whitespace", async () => {
    const text = "Hello 中文 😀\n", bytes = new TextEncoder().encode(text)
    const encoded = btoa(String.fromCharCode(...bytes)).replace(/.{4}/g, "$& \n")
    const { onOsc } = await openOutputDialog()
    await act(async () => { expect(onOsc("c;" + encoded)).toBe(true) })
    expect(mocks.oscHandler).toHaveBeenCalledWith(52, expect.any(Function))
    expect(mocks.writeText).toHaveBeenCalledExactlyOnceWith(text)
  })

  it.each(["query", "malformed", "oversized"])("ignores %s OSC52 data", async kind => {
    const encoded = kind === "query" ? "?" : kind === "malformed" ? "%%%" : "A".repeat(4 * 1024 * 1024 + 4)
    const { onOsc } = await openOutputDialog()
    await act(async () => { expect(onOsc("c;" + encoded)).toBe(true) })
    expect(mocks.writeText).not.toHaveBeenCalled()
    expect(screen.queryByRole("alert")).not.toBeInTheDocument()
  })

  it.each([0, 4 * 1024 * 1024])("accepts the OSC52 encoded boundary of %i characters", async size => {
    const { onOsc } = await openOutputDialog()
    await act(async () => { expect(onOsc("c;" + "A".repeat(size))).toBe(true) })
    expect(mocks.writeText).toHaveBeenCalledExactlyOnceWith("\0".repeat(size / 4 * 3))
  })

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

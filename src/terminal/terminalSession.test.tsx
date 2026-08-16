import React from "react"
import { fireEvent, render, screen, waitFor } from "@testing-library/react"
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"

import type { PtyEvent } from "../lib/types"

const xtermMock = vi.hoisted(() => {
    type DataHandler = (data: string) => void
    type KeyHandler = (event: KeyboardEvent) => boolean
    type TitleHandler = (title: string) => void

    const state = {
        terminals: [] as TerminalMock[],
        fits: [] as FitAddonMock[],
        fitDimensions: { cols: 100, rows: 30 }
    }

    class TerminalMock {
        options: Record<string, unknown>
        cols = 80
        rows = 24
        selection = ""
        dataHandler: DataHandler | null = null
        keyHandler: KeyHandler | null = null
        titleHandler: TitleHandler | null = null
        writeParsedHandler: (() => void) | null = null
        titleDisposable = { dispose: vi.fn() }
        linkProvider: unknown = null
        linkProviderDisposable = { dispose: vi.fn() }
        element = document.createElement("div")
        open = vi.fn()
        write = vi.fn((_data: string, onProcessed?: () => void) => onProcessed?.())
        focus = vi.fn()
        refresh = vi.fn()
        hasSelection = vi.fn(() => this.selection.length > 0)
        getSelection = vi.fn(() => this.selection)
        paste = vi.fn((text: string) => this.dataHandler?.(text))
        clear = vi.fn()
        dispose = vi.fn()
        loadAddon = vi.fn((addon: { activate?: (terminal: TerminalMock) => void }) => {
            addon.activate?.(this)
        })
        onData = vi.fn((handler: DataHandler) => {
            this.dataHandler = handler
            return { dispose: vi.fn() }
        })
        attachCustomKeyEventHandler = vi.fn((handler: KeyHandler) => {
            this.keyHandler = handler
        })
        onWriteParsed = vi.fn((handler: () => void) => {
            this.writeParsedHandler = handler
            return { dispose: vi.fn() }
        })
        onTitleChange = vi.fn((handler: TitleHandler) => {
            this.titleHandler = handler
            return this.titleDisposable
        })
        registerLinkProvider = vi.fn((provider: unknown) => {
            this.linkProvider = provider
            return this.linkProviderDisposable
        })

        constructor(options: Record<string, unknown>) {
            this.options = options
            state.terminals.push(this)
        }

        emitData(data: string) {
            this.dataHandler?.(data)
        }

        emitKey(event: KeyboardEvent) {
            return this.keyHandler?.(event) ?? true
        }

        emitTitle(title: string) {
            this.titleHandler?.(title)
        }
    }

    class FitAddonMock {
        terminal: TerminalMock | null = null
        activate = vi.fn((terminal: TerminalMock) => {
            this.terminal = terminal
        })
        dispose = vi.fn()
        fit = vi.fn(() => {
            if (!this.terminal) return
            this.terminal.cols = state.fitDimensions.cols
            this.terminal.rows = state.fitDimensions.rows
        })
        proposeDimensions = vi.fn(() => state.fitDimensions)

        constructor() {
            state.fits.push(this)
        }
    }

    return { state, TerminalMock, FitAddonMock }
})

const ipcMock = vi.hoisted(() => ({
    onEvent: null as ((event: PtyEvent) => void) | null,
    ptyOpen: vi.fn(),
    ptyWrite: vi.fn(),
    ptyResize: vi.fn(),
    ptyClose: vi.fn()
}))

const clipboardMock = vi.hoisted(() => ({
    readText: vi.fn(),
    writeText: vi.fn()
}))

const imeMock = vi.hoisted(() => {
    const state = { disposables: [] as Array<{ dispose: ReturnType<typeof vi.fn> }> }
    return {
        state,
        install: vi.fn(() => {
            const disposable = { dispose: vi.fn() }
            state.disposables.push(disposable)
            return disposable
        })
    }
})

vi.mock("@xterm/xterm", () => ({
    Terminal: xtermMock.TerminalMock
}))

vi.mock("@xterm/addon-fit", () => ({
    FitAddon: xtermMock.FitAddonMock
}))

vi.mock("@tauri-apps/plugin-clipboard-manager", () => clipboardMock)

vi.mock("./terminalImePositioning", () => ({
    installTerminalImePositioning: imeMock.install
}))

vi.mock("../lib/ipc", () => ({
    ptyOpen: ipcMock.ptyOpen,
    ptyWrite: ipcMock.ptyWrite,
    ptyResize: ipcMock.ptyResize,
    ptyClose: ipcMock.ptyClose
}))

import { TerminalSession } from "./TerminalSession"
import { terminalOutputLossNotice, terminalOutputMetrics } from "./terminalOutputQueue"
import { useTerminalSettingsStore } from "../state/terminalSettingsStore"

function outputEvent(data: string, seq = 0, droppedBytes = 0): PtyEvent {
    return { type: "output", data, seq, droppedBytes, truncated: droppedBytes > 0 }
}

type ResizeObserverMock = {
    observe: ReturnType<typeof vi.fn>
    disconnect: ReturnType<typeof vi.fn>
    trigger: () => void
}

const resizeObservers: ResizeObserverMock[] = []

beforeEach(() => {
    xtermMock.state.terminals.length = 0
    xtermMock.state.fits.length = 0
    imeMock.state.disposables.length = 0
    xtermMock.state.fitDimensions = { cols: 100, rows: 30 }
    ipcMock.onEvent = null
    ipcMock.ptyOpen.mockImplementation(
        async (
            workspace: string,
            sessionId: string,
            shell: string | null,
            _shellArgs: string[] | undefined,
            _cwdStrategy: "native" | "wsl",
            cols: number,
            rows: number,
            onEvent: (event: PtyEvent) => void
        ) => {
            ipcMock.onEvent = onEvent
            return { workspace, sessionId, shell: shell ?? "/bin/zsh", cols, rows }
        }
    )
    ipcMock.ptyWrite.mockResolvedValue(undefined)
    ipcMock.ptyResize.mockResolvedValue(undefined)
    ipcMock.ptyClose.mockResolvedValue(undefined)
    useTerminalSettingsStore.setState({ fontSize: 12, imeAnchorMode: "cursor" })
    clipboardMock.readText.mockResolvedValue("")
    clipboardMock.writeText.mockResolvedValue(undefined)
    resizeObservers.length = 0

    globalThis.ResizeObserver = class {
        observe = vi.fn()
        disconnect = vi.fn()
        trigger: () => void

        constructor(callback: ResizeObserverCallback) {
            this.trigger = () => callback([], this as unknown as ResizeObserver)
            resizeObservers.push(this)
        }
    } as unknown as typeof ResizeObserver
})

afterEach(() => {
    vi.clearAllMocks()
})

describe("TerminalSession", () => {
    it("opens one xterm and pty session under StrictMode", async () => {
        render(
            <React.StrictMode>
                <TerminalSession workspace="/w" sessionId="pty-1" shell={null} active={false} />
            </React.StrictMode>
        )

        expect(screen.getByTestId("terminal-session-pty-1")).toBeInTheDocument()

        await waitFor(() => expect(ipcMock.ptyOpen).toHaveBeenCalledTimes(1))
        expect(ipcMock.ptyOpen).toHaveBeenCalledWith(
            "/w",
            "pty-1",
            null,
            undefined,
            "native",
            100,
            30,
            expect.any(Function)
        )
        expect(xtermMock.state.terminals).toHaveLength(1)
        expect(xtermMock.state.terminals[0].options.fontSize).toBe(12)
        expect(xtermMock.state.terminals[0].registerLinkProvider).toHaveBeenCalledTimes(1)
        expect(xtermMock.state.terminals[0].options.linkHandler).toMatchObject({
            activate: expect.any(Function),
            hover: expect.any(Function),
            leave: expect.any(Function),
            allowNonHttpProtocols: false
        })
        expect(xtermMock.state.terminals[0].open).toHaveBeenCalled()
        expect(xtermMock.state.fits[0].fit).toHaveBeenCalled()
        fireEvent.mouseLeave(screen.getByTestId("terminal-session-pty-1").firstElementChild!)
        expect(xtermMock.state.terminals[0].refresh).toHaveBeenCalledWith(0, 29)
        expect(imeMock.install).toHaveBeenCalledWith(
            xtermMock.state.terminals[0],
            { anchorMode: "cursor" }
        )
    })

    it("resets stale link hover after parsed output, mouseleave, and window blur", async () => {
        render(<TerminalSession workspace="/w" sessionId="pty-hover" active visible />)
        await waitFor(() => expect(xtermMock.state.terminals).toHaveLength(1))
        const term = xtermMock.state.terminals[0]

        term.writeParsedHandler?.()
        fireEvent.mouseLeave(screen.getByTestId("terminal-session-pty-hover").firstElementChild!)
        fireEvent.blur(window)

        expect(term.onWriteParsed).toHaveBeenCalledTimes(1)
        expect(term.element.style.cursor).toBe("")
    })

    it("wires terminal input to ptyWrite and pty output to term.write", async () => {
        render(<TerminalSession workspace="/w" sessionId="pty-2" shell="/bin/fish" active={false} />)

        await waitFor(() => expect(ipcMock.ptyOpen).toHaveBeenCalledTimes(1))
        xtermMock.state.terminals[0].emitData("pwd\n")
        expect(ipcMock.ptyWrite).toHaveBeenCalledWith("pty-2", "pwd\n")

        ipcMock.onEvent?.(outputEvent("ready\n"))
        await waitFor(() => expect(xtermMock.state.terminals[0].write).toHaveBeenCalledWith(
            "ready\n",
            expect.any(Function)
        ))
    })

    it("marks output the backend pending cap dropped", async () => {
        render(<TerminalSession workspace="/w" sessionId="pty-dropped" active={false} />)

        await waitFor(() => expect(ipcMock.ptyOpen).toHaveBeenCalledTimes(1))
        ipcMock.onEvent?.(outputEvent("tail\n", 4, 4096))

        await waitFor(() => expect(xtermMock.state.terminals[0].write).toHaveBeenCalledWith(
            `${terminalOutputLossNotice(4096, 0)}tail\n`,
            expect.any(Function)
        ))
    })

    it("does not mark output when the backend reports no drop", async () => {
        render(<TerminalSession workspace="/w" sessionId="pty-nodrop" active={false} />)

        await waitFor(() => expect(ipcMock.ptyOpen).toHaveBeenCalledTimes(1))
        ipcMock.onEvent?.(outputEvent("clean\n", 1))

        await waitFor(() => expect(xtermMock.state.terminals[0].write).toHaveBeenCalledWith(
            "clean\n",
            expect.any(Function)
        ))
    })

    it("marks whole output events lost when the backend sequence skips", async () => {
        render(<TerminalSession workspace="/w" sessionId="pty-gap" active={false} />)

        await waitFor(() => expect(ipcMock.ptyOpen).toHaveBeenCalledTimes(1))
        ipcMock.onEvent?.(outputEvent("first\n", 0))
        await waitFor(() => expect(xtermMock.state.terminals[0].write).toHaveBeenCalledWith(
            "first\n",
            expect.any(Function)
        ))

        // seq jumps 0 -> 3, so events 1 and 2 never arrived.
        ipcMock.onEvent?.(outputEvent("fourth\n", 3))
        await waitFor(() => expect(xtermMock.state.terminals[0].write).toHaveBeenCalledWith(
            `${terminalOutputLossNotice(0, 2)}fourth\n`,
            expect.any(Function)
        ))
    })

    it("does not report a gap for contiguous sequence numbers", async () => {
        render(<TerminalSession workspace="/w" sessionId="pty-contiguous" active={false} />)

        await waitFor(() => expect(ipcMock.ptyOpen).toHaveBeenCalledTimes(1))
        ipcMock.onEvent?.(outputEvent("a\n", 7))
        ipcMock.onEvent?.(outputEvent("b\n", 8))

        await waitFor(() => expect(xtermMock.state.terminals[0].write).toHaveBeenCalledWith(
            "a\nb\n",
            expect.any(Function)
        ))
    })

    it("keeps both markers visible when a storm-sized event arrives", async () => {
        render(<TerminalSession workspace="/w" sessionId="pty-storm" active={false} />)
        await waitFor(() => expect(ipcMock.ptyOpen).toHaveBeenCalledTimes(1))

        ipcMock.onEvent?.(outputEvent("seed\n", 0))
        await waitFor(() => expect(xtermMock.state.terminals[0].write).toHaveBeenCalled())
        xtermMock.state.terminals[0].write.mockClear()

        // A full backend batch that overflows the frontend ring buffer, with a
        // sequence gap on top: both markers must survive the eviction.
        const payload = "z".repeat(1024 * 1024)
        ipcMock.onEvent?.(outputEvent(payload, 3, 4096))

        await waitFor(() => expect(xtermMock.state.terminals[0].write).toHaveBeenCalled())
        const written = xtermMock.state.terminals[0].write.mock.calls
            .map((call) => call[0] as string)
            .join("")
        expect(written).toContain(terminalOutputLossNotice(4096, 2))
        expect(written.endsWith("z".repeat(64))).toBe(true)
    })

    it("publishes queue metrics for the session through the registry", async () => {
        expect(terminalOutputMetrics("pty-metrics")).toBeNull()

        const { unmount } = render(
            <TerminalSession workspace="/w" sessionId="pty-metrics" active={false} />
        )
        await waitFor(() => expect(ipcMock.ptyOpen).toHaveBeenCalledTimes(1))

        expect(terminalOutputMetrics("pty-metrics")).not.toBeNull()
        ipcMock.onEvent?.(outputEvent("hello\n", 0))
        await waitFor(() =>
            expect(terminalOutputMetrics("pty-metrics")?.flushCount).toBeGreaterThan(0)
        )

        unmount()
        await waitFor(() => expect(terminalOutputMetrics("pty-metrics")).toBeNull())
    })

    it("reports OSC title, ready, and spawn failure lifecycle without owning title policy", async () => {
        const onTitleChange = vi.fn()
        const onReady = vi.fn()
        render(
            <TerminalSession
                workspace="/w"
                sessionId="pty-lifecycle"
                active={false}
                onTitleChange={onTitleChange}
                onReady={onReady}
            />
        )

        await waitFor(() => expect(onReady).toHaveBeenCalledTimes(1))
        xtermMock.state.terminals[0].emitTitle("dev server")
        expect(onTitleChange).toHaveBeenCalledWith("dev server")

        ipcMock.ptyOpen.mockRejectedValueOnce(new Error("shell missing"))
        const onOpenError = vi.fn()
        render(
            <TerminalSession
                workspace="/w"
                sessionId="pty-failed"
                active={false}
                onOpenError={onOpenError}
            />
        )
        await waitFor(() => expect(onOpenError).toHaveBeenCalledWith("shell missing"))
        await waitFor(() => expect(xtermMock.state.terminals[1].write).toHaveBeenCalledWith(
            "\r\n[Failed to open terminal: shell missing]\r\n",
            expect.any(Function)
        ))
    })

    it("keeps one PTY and xterm while hidden, then refits and focuses when shown again", async () => {
        const { rerender } = render(
            <TerminalSession
                workspace="/w"
                sessionId="pty-hidden"
                active={false}
                visible
            />
        )
        await waitFor(() => expect(ipcMock.ptyOpen).toHaveBeenCalledTimes(1))
        const term = xtermMock.state.terminals[0]

        rerender(
            <TerminalSession
                workspace="/w"
                sessionId="pty-hidden"
                active={false}
                visible={false}
            />
        )
        xtermMock.state.fitDimensions = { cols: 140, rows: 40 }
        resizeObservers[0].trigger()
        expect(ipcMock.ptyResize).not.toHaveBeenCalled()
        ipcMock.onEvent?.(outputEvent("hidden output\n"))
        expect(term.write).not.toHaveBeenCalled()

        rerender(
            <TerminalSession
                workspace="/w"
                sessionId="pty-hidden"
                active
                visible
            />
        )
        await waitFor(() => expect(term.focus).toHaveBeenCalled())
        await waitFor(() => expect(term.write).toHaveBeenCalledWith(
            "hidden output\n",
            expect.any(Function)
        ))
        expect(ipcMock.ptyOpen).toHaveBeenCalledTimes(1)
        expect(xtermMock.state.terminals).toHaveLength(1)
        expect(ipcMock.ptyResize).toHaveBeenCalledWith("pty-hidden", 140, 40)
    })

    it("updates the font size live and suppresses the native viewport context menu", async () => {
        render(<TerminalSession workspace="/w" sessionId="pty-font" active />)
        await waitFor(() => expect(ipcMock.ptyOpen).toHaveBeenCalledTimes(1))
        const surface = screen.getByTestId("terminal-session-pty-font")
        const event = new MouseEvent("contextmenu", { bubbles: true, cancelable: true })

        surface.dispatchEvent(event)
        expect(event.defaultPrevented).toBe(true)

        useTerminalSettingsStore.getState().update({ fontSize: 18 })
        await waitFor(() => expect(xtermMock.state.terminals[0].options.fontSize).toBe(18))
        expect(xtermMock.state.fits[0].fit).toHaveBeenCalled()
    })

    it.each([
        ["Ctrl", { ctrlKey: true }],
        ["Cmd", { metaKey: true }]
    ])("copies the selected text with %s+C", async (_label, modifier) => {
        render(<TerminalSession workspace="/w" sessionId="pty-copy" active={false} />)
        await waitFor(() => expect(ipcMock.ptyOpen).toHaveBeenCalledTimes(1))
        const term = xtermMock.state.terminals[0]
        term.selection = "selected terminal text"
        const event = new KeyboardEvent("keydown", {
            key: "c",
            cancelable: true,
            ...modifier
        })

        expect(term.emitKey(event)).toBe(false)
        await waitFor(() => {
            expect(clipboardMock.writeText).toHaveBeenCalledWith("selected terminal text")
        })
        expect(event.defaultPrevented).toBe(true)
        expect(ipcMock.ptyWrite).not.toHaveBeenCalled()
    })

    it("keeps Ctrl+C available to the shell when no text is selected", async () => {
        render(<TerminalSession workspace="/w" sessionId="pty-interrupt" active={false} />)
        await waitFor(() => expect(ipcMock.ptyOpen).toHaveBeenCalledTimes(1))
        const event = new KeyboardEvent("keydown", { key: "c", ctrlKey: true })

        expect(xtermMock.state.terminals[0].emitKey(event)).toBe(true)
        expect(clipboardMock.writeText).not.toHaveBeenCalled()
    })

    it.each([
        ["Ctrl", { ctrlKey: true }],
        ["Cmd", { metaKey: true }]
    ])("pastes clipboard text through xterm with %s+V", async (_label, modifier) => {
        clipboardMock.readText.mockResolvedValue("clipboard payload")
        render(<TerminalSession workspace="/w" sessionId="pty-paste" active={false} />)
        await waitFor(() => expect(ipcMock.ptyOpen).toHaveBeenCalledTimes(1))
        const event = new KeyboardEvent("keydown", {
            key: "v",
            cancelable: true,
            ...modifier
        })

        expect(xtermMock.state.terminals[0].emitKey(event)).toBe(false)
        await waitFor(() => {
            expect(ipcMock.ptyWrite).toHaveBeenCalledWith("pty-paste", "clipboard payload")
        })
        expect(event.defaultPrevented).toBe(true)
        expect(xtermMock.state.terminals[0].paste).toHaveBeenCalledWith("clipboard payload")
    })

    it("renders exited state and invokes onExit when the pty exits", async () => {
        const onExit = vi.fn()

        render(<TerminalSession workspace="/w" sessionId="pty-3" active={false} onExit={onExit} />)
        await waitFor(() => expect(ipcMock.ptyOpen).toHaveBeenCalledTimes(1))

        ipcMock.onEvent?.({ type: "exit", code: 7 })

        expect(await screen.findByRole("status")).toHaveTextContent("Exited 7")
        expect(onExit).toHaveBeenCalledWith(7)
    })

    it("fits and sends ptyResize when the container resizes", async () => {
        render(<TerminalSession workspace="/w" sessionId="pty-4" active={false} />)
        await waitFor(() => expect(ipcMock.ptyOpen).toHaveBeenCalledTimes(1))

        xtermMock.state.fitDimensions = { cols: 120, rows: 34 }
        resizeObservers[0].trigger()

        expect(xtermMock.state.fits[0].fit).toHaveBeenCalledTimes(2)
        expect(ipcMock.ptyResize).toHaveBeenCalledWith("pty-4", 120, 34)
    })

    it("focuses active sessions and cleans up on unmount", async () => {
        const { unmount } = render(<TerminalSession workspace="/w" sessionId="pty-5" active />)
        await waitFor(() => expect(ipcMock.ptyOpen).toHaveBeenCalledTimes(1))

        expect(xtermMock.state.terminals[0].focus).toHaveBeenCalled()

        unmount()

        await waitFor(() => expect(xtermMock.state.terminals[0].dispose).toHaveBeenCalled())
        expect(xtermMock.state.fits[0].dispose).toHaveBeenCalled()
        expect(xtermMock.state.terminals[0].titleDisposable.dispose).toHaveBeenCalledTimes(1)
        expect(xtermMock.state.terminals[0].linkProviderDisposable.dispose).toHaveBeenCalledTimes(1)
        expect(imeMock.state.disposables[0].dispose).toHaveBeenCalledTimes(1)
        expect(resizeObservers[0].disconnect).toHaveBeenCalled()
        expect(ipcMock.ptyClose).toHaveBeenCalledWith("pty-5")
    })

    it("closes again after delayed ptyOpen settles and ignores late events after dispose", async () => {
        let resolveOpen!: () => void
        const openPromise = new Promise<void>((resolve) => {
            resolveOpen = resolve
        })
        ipcMock.ptyOpen.mockImplementation(
            async (
                _workspace: string,
                _sessionId: string,
                _shell: string | null,
                _shellArgs: string[] | undefined,
                _cwdStrategy: "native" | "wsl",
                _cols: number,
                _rows: number,
                onEvent: (event: PtyEvent) => void
            ) => {
                ipcMock.onEvent = onEvent
                await openPromise
            }
        )

        const { unmount } = render(<TerminalSession workspace="/w" sessionId="pty-6" active={false} />)
        await waitFor(() => expect(ipcMock.ptyOpen).toHaveBeenCalledTimes(1))

        unmount()
        await waitFor(() => expect(ipcMock.ptyClose).toHaveBeenCalledTimes(1))
        expect(ipcMock.ptyClose).toHaveBeenLastCalledWith("pty-6")

        resolveOpen()
        await waitFor(() => expect(ipcMock.ptyClose).toHaveBeenCalledTimes(2))
        expect(ipcMock.ptyClose).toHaveBeenLastCalledWith("pty-6")

        ipcMock.onEvent?.(outputEvent("late\n"))
        expect(xtermMock.state.terminals[0].write).not.toHaveBeenCalled()
    })

    it("passes shell args to ptyOpen", async () => {
        render(
            <TerminalSession
                workspace="/w"
                sessionId="pty-7"
                shell="/bin/sh"
                shellArgs={["-c", "echo ok"]}
                active={false}
            />
        )

        await waitFor(() => expect(ipcMock.ptyOpen).toHaveBeenCalledTimes(1))
        expect(ipcMock.ptyOpen).toHaveBeenCalledWith(
            "/w",
            "pty-7",
            "/bin/sh",
            ["-c", "echo ok"],
            "native",
            100,
            30,
            expect.any(Function)
        )
    })
})

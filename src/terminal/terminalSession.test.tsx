import React from "react"
import { render, screen, waitFor } from "@testing-library/react"
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"

import type { PtyEvent } from "../lib/types"

const xtermMock = vi.hoisted(() => {
    type DataHandler = (data: string) => void

    const state = {
        terminals: [] as TerminalMock[],
        fits: [] as FitAddonMock[],
        fitDimensions: { cols: 100, rows: 30 }
    }

    class TerminalMock {
        options: Record<string, unknown>
        cols = 80
        rows = 24
        dataHandler: DataHandler | null = null
        open = vi.fn()
        write = vi.fn()
        focus = vi.fn()
        dispose = vi.fn()
        loadAddon = vi.fn((addon: { activate?: (terminal: TerminalMock) => void }) => {
            addon.activate?.(this)
        })
        onData = vi.fn((handler: DataHandler) => {
            this.dataHandler = handler
            return { dispose: vi.fn() }
        })

        constructor(options: Record<string, unknown>) {
            this.options = options
            state.terminals.push(this)
        }

        emitData(data: string) {
            this.dataHandler?.(data)
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

vi.mock("@xterm/xterm", () => ({
    Terminal: xtermMock.TerminalMock
}))

vi.mock("@xterm/addon-fit", () => ({
    FitAddon: xtermMock.FitAddonMock
}))

vi.mock("../lib/ipc", () => ({
    ptyOpen: ipcMock.ptyOpen,
    ptyWrite: ipcMock.ptyWrite,
    ptyResize: ipcMock.ptyResize,
    ptyClose: ipcMock.ptyClose
}))

import { TerminalSession } from "./TerminalSession"

type ResizeObserverMock = {
    observe: ReturnType<typeof vi.fn>
    disconnect: ReturnType<typeof vi.fn>
    trigger: () => void
}

const resizeObservers: ResizeObserverMock[] = []

beforeEach(() => {
    xtermMock.state.terminals.length = 0
    xtermMock.state.fits.length = 0
    xtermMock.state.fitDimensions = { cols: 100, rows: 30 }
    ipcMock.onEvent = null
    ipcMock.ptyOpen.mockImplementation(
        async (
            workspace: string,
            sessionId: string,
            shell: string | null,
            _shellArgs: string[] | undefined,
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
            100,
            30,
            expect.any(Function)
        )
        expect(xtermMock.state.terminals).toHaveLength(1)
        expect(xtermMock.state.terminals[0].options.fontSize).toBe(12)
        expect(xtermMock.state.terminals[0].open).toHaveBeenCalled()
        expect(xtermMock.state.fits[0].fit).toHaveBeenCalled()
    })

    it("wires terminal input to ptyWrite and pty output to term.write", async () => {
        render(<TerminalSession workspace="/w" sessionId="pty-2" shell="/bin/fish" active={false} />)

        await waitFor(() => expect(ipcMock.ptyOpen).toHaveBeenCalledTimes(1))
        xtermMock.state.terminals[0].emitData("pwd\n")
        expect(ipcMock.ptyWrite).toHaveBeenCalledWith("pty-2", "pwd\n")

        ipcMock.onEvent?.({ type: "output", data: "ready\n" })
        expect(xtermMock.state.terminals[0].write).toHaveBeenCalledWith("ready\n")
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

        ipcMock.onEvent?.({ type: "output", data: "late\n" })
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
            100,
            30,
            expect.any(Function)
        )
    })
})

import React from "react"
import { render, waitFor } from "@testing-library/react"
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"

// Regression coverage for the SSH shell-open review finding: openedShells used
// to be marked *before* sshOpenShell resolved and never cleared on failure, so a
// failed open (server refuses PTY, first StrictMode mount fails) left the id
// pinned forever and every later remount skipped the reopen. The guard must
// still open exactly once for a live session, but a *failed* open must be
// retriable by a remount.

const xtermMock = vi.hoisted(() => {
    type DataHandler = (data: string) => void

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
    }

    class FitAddonMock {
        terminal: TerminalMock | null = null
        activate = vi.fn((terminal: TerminalMock) => {
            this.terminal = terminal
        })
        dispose = vi.fn()
        fit = vi.fn()
    }

    const state = { terminals: [] as TerminalMock[] }
    return { state, TerminalMock, FitAddonMock }
})

vi.mock("@xterm/xterm", () => ({ Terminal: xtermMock.TerminalMock }))
vi.mock("@xterm/addon-fit", () => ({ FitAddon: xtermMock.FitAddonMock }))

vi.mock("@tauri-apps/api/event", () => ({
    listen: vi.fn(async () => () => undefined)
}))

const ipcMock = vi.hoisted(() => ({
    sshOpenShell: vi.fn(),
    sshWrite: vi.fn(),
    sshResize: vi.fn()
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

vi.mock("@/lib/ipc", () => ipcMock)
vi.mock("./terminalImePositioning", () => ({
    installTerminalImePositioning: imeMock.install
}))

import { SshTerminalSession } from "./SshTerminalSession"

class ResizeObserverStub {
    observe = vi.fn()
    disconnect = vi.fn()
    unobserve = vi.fn()
}

beforeEach(() => {
    xtermMock.state.terminals.length = 0
    imeMock.state.disposables.length = 0
    vi.clearAllMocks()
    globalThis.ResizeObserver = ResizeObserverStub as unknown as typeof ResizeObserver
    ipcMock.sshOpenShell.mockResolvedValue(undefined)
    ipcMock.sshWrite.mockResolvedValue(undefined)
    ipcMock.sshResize.mockResolvedValue(undefined)
})

afterEach(() => {
    vi.clearAllMocks()
})

describe("SshTerminalSession shell-open guard", () => {
    it("opens the shell exactly once under a StrictMode double mount", async () => {
        render(
            <React.StrictMode>
                <SshTerminalSession sessionId="ssh-strict" active={false} />
            </React.StrictMode>
        )

        await waitFor(() => expect(ipcMock.sshOpenShell).toHaveBeenCalledTimes(1))
        // A second mount from StrictMode must not spawn a second shell channel.
        await Promise.resolve()
        expect(ipcMock.sshOpenShell).toHaveBeenCalledTimes(1)
        expect(ipcMock.sshOpenShell).toHaveBeenCalledWith("ssh-strict", 80, 24)
        expect(imeMock.install).toHaveBeenCalledWith(
            xtermMock.state.terminals[0],
            { anchorMode: "cursor" }
        )
    })

    it("does not reopen the shell when a live session remounts", async () => {
        const { unmount } = render(<SshTerminalSession sessionId="ssh-live" active={false} />)
        await waitFor(() => expect(ipcMock.sshOpenShell).toHaveBeenCalledTimes(1))

        unmount()
        render(<SshTerminalSession sessionId="ssh-live" active={false} />)

        // The successful open is remembered — the remount reuses the live shell.
        await Promise.resolve()
        expect(ipcMock.sshOpenShell).toHaveBeenCalledTimes(1)
    })

    it("retries the shell open on remount after a failed open", async () => {
        ipcMock.sshOpenShell.mockRejectedValueOnce(new Error("PTY denied"))

        const { unmount } = render(<SshTerminalSession sessionId="ssh-retry" active={false} />)
        await waitFor(() => expect(ipcMock.sshOpenShell).toHaveBeenCalledTimes(1))
        // The failure path writes an error notice AND clears the guard marker.
        await waitFor(() =>
            expect(xtermMock.state.terminals[0].write).toHaveBeenCalledWith(
                expect.stringContaining("Failed to open shell")
            )
        )

        unmount()
        render(<SshTerminalSession sessionId="ssh-retry" active={false} />)

        // Marker was cleared on failure, so the remount reopens instead of
        // being skipped forever.
        await waitFor(() => expect(ipcMock.sshOpenShell).toHaveBeenCalledTimes(2))
    })

    it("disposes IME positioning with the xterm session", async () => {
        const { unmount } = render(<SshTerminalSession sessionId="ssh-ime" active={false} />)
        await waitFor(() => expect(ipcMock.sshOpenShell).toHaveBeenCalledTimes(1))

        unmount()

        expect(imeMock.state.disposables[0].dispose).toHaveBeenCalledTimes(1)
    })
})

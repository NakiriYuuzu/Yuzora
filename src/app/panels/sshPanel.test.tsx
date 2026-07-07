import { render, screen, cleanup, fireEvent, act } from "@testing-library/react"
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"

// Regression coverage for the two SSH panel review findings:
//  1. the TOFU host-key fingerprint must actually render somewhere (it used
//     to only be stored in state, never shown).
//  2. switching the active host between two *connected* hosts must not
//     dispose/recreate either xterm instance (that wiped scrollback).

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

const listenMock = vi.hoisted(() => {
    const listeners = new Map<string, Set<(e: { payload: unknown }) => void>>()
    return {
        listeners,
        emit(event: string, payload: unknown) {
            for (const cb of listeners.get(event) ?? []) cb({ payload })
        }
    }
})

vi.mock("@tauri-apps/api/event", () => ({
    listen: vi.fn(async (event: string, cb: (e: { payload: unknown }) => void) => {
        if (!listenMock.listeners.has(event)) listenMock.listeners.set(event, new Set())
        listenMock.listeners.get(event)!.add(cb)
        return () => {
            listenMock.listeners.get(event)?.delete(cb)
        }
    })
}))

const ipcMock = vi.hoisted(() => ({
    sshConnect: vi.fn(),
    sshDisconnect: vi.fn(),
    sshOpenShell: vi.fn(),
    sshWrite: vi.fn(),
    sshResize: vi.fn(),
    listDir: vi.fn(),
    sftpListDir: vi.fn(),
    sftpMkdir: vi.fn(),
    sftpRename: vi.fn(),
    sftpRemove: vi.fn(),
    sftpUpload: vi.fn(),
    sftpDownload: vi.fn()
}))

vi.mock("@/lib/ipc", () => ipcMock)

// Capture the drag-drop handler so a test can drive an OS-file drop (Phase 1).
const dragMock = vi.hoisted(() => ({ handler: null as ((e: unknown) => void) | null }))
vi.mock("@tauri-apps/api/webview", () => ({
    getCurrentWebview: () => ({
        onDragDropEvent: (h: (e: unknown) => void) => {
            dragMock.handler = h
            return Promise.resolve(() => {
                dragMock.handler = null
            })
        }
    })
}))

vi.mock("@tauri-apps/plugin-dialog", () => ({
    open: vi.fn(async () => null),
    save: vi.fn(async () => null)
}))

import { SshPanel } from "./SshPanel"
import { useSshStore } from "@/state/sshStore"
import { useSftpStore } from "@/state/sftpStore"
import { useWorkspaceStore } from "@/state/workspaceStore"

// The Bun-hosted test runtime injects an empty `localStorage` global with no
// Storage methods; install a minimal in-memory Storage so sshStore's
// load/save round-trip runs for real (mirrors sshStore.test.ts).
function installLocalStorage(): void {
    const store = new Map<string, string>()
    const mock = {
        getItem: (k: string) => (store.has(k) ? store.get(k)! : null),
        setItem: (k: string, v: string) => void store.set(k, String(v)),
        removeItem: (k: string) => void store.delete(k),
        clear: () => store.clear(),
        key: (i: number) => [...store.keys()][i] ?? null,
        get length() {
            return store.size
        }
    }
    Object.defineProperty(globalThis, "localStorage", {
        value: mock,
        configurable: true,
        writable: true
    })
}

class ResizeObserverStub {
    observe = vi.fn()
    disconnect = vi.fn()
    unobserve = vi.fn()
}

let sessionSeq = 0

beforeEach(() => {
    installLocalStorage()
    localStorage.clear()
    xtermMock.state.terminals.length = 0
    listenMock.listeners.clear()
    vi.clearAllMocks()
    globalThis.ResizeObserver = ResizeObserverStub as unknown as typeof ResizeObserver

    // Not reset per-test: sessionIds must stay unique across the whole file
    // because the SshTerminalSession module's `openedShells` guard is a
    // module-level singleton keyed by sessionId, not reset between tests.
    ipcMock.sshConnect.mockImplementation(async () => {
        sessionSeq += 1
        return { sessionId: `sess-${sessionSeq}`, fingerprint: `SHA256:host-${sessionSeq}` }
    })
    ipcMock.sshDisconnect.mockResolvedValue(undefined)
    ipcMock.sshOpenShell.mockResolvedValue(undefined)
    ipcMock.sshWrite.mockResolvedValue(undefined)
    ipcMock.sshResize.mockResolvedValue(undefined)
    ipcMock.listDir.mockResolvedValue([])
    ipcMock.sftpListDir.mockResolvedValue({ cwd: "/home/u", entries: [] })
    ipcMock.sftpMkdir.mockResolvedValue(undefined)
    ipcMock.sftpRename.mockResolvedValue(undefined)
    ipcMock.sftpRemove.mockResolvedValue(undefined)
    ipcMock.sftpUpload.mockResolvedValue(undefined)
    ipcMock.sftpDownload.mockResolvedValue(undefined)
    dragMock.handler = null

    useSshStore.setState({ hosts: [], sessions: {}, activeHostId: null, pendingAuthHostId: null })
    useSftpStore.getState().reset()
    useWorkspaceStore.setState({ workspacePath: null })
})

afterEach(() => {
    cleanup()
})

async function connectTwoHosts() {
    const hostA = useSshStore.getState().addHost({
        name: "a",
        host: "a.example.com",
        port: 22,
        user: "root",
        authKind: "password"
    })
    const hostB = useSshStore.getState().addHost({
        name: "b",
        host: "b.example.com",
        port: 22,
        user: "root",
        authKind: "password"
    })
    await useSshStore.getState().connect(hostA.id, "pw-a")
    await useSshStore.getState().connect(hostB.id, "pw-b")
    const sessionIdA = useSshStore.getState().sessions[hostA.id]!.sessionId!
    const sessionIdB = useSshStore.getState().sessions[hostB.id]!.sessionId!
    const fingerprintB = useSshStore.getState().sessions[hostB.id]!.fingerprint!
    return { hostA, hostB, sessionIdA, sessionIdB, fingerprintB }
}

describe("SshPanel fingerprint display", () => {
    it("renders the TOFU host-key fingerprint for the active session", async () => {
        const { fingerprintB } = await connectTwoHosts()
        render(<SshPanel />)

        // hostB connected last, so it's active — its fingerprint must be visible.
        expect(await screen.findByText(new RegExp(fingerprintB))).toBeInTheDocument()
    })

    it("dismiss button hides the notice for that session only", async () => {
        const { fingerprintB } = await connectTwoHosts()
        render(<SshPanel />)

        const dismiss = await screen.findByRole("button", { name: "Dismiss host key notice" })
        fireEvent.click(dismiss)
        expect(screen.queryByText(new RegExp(fingerprintB))).not.toBeInTheDocument()
    })
})

describe("SshPanel multi-host terminal mounting", () => {
    it("keeps both connected hosts' xterm instances alive across an active-host switch", async () => {
        const { hostA, sessionIdA, sessionIdB } = await connectTwoHosts()
        render(<SshPanel />)

        expect(await screen.findByTestId(`ssh-terminal-session-${sessionIdB}`)).toBeInTheDocument()
        expect(xtermMock.state.terminals).toHaveLength(2)
        expect(ipcMock.sshOpenShell).toHaveBeenCalledTimes(2)

        // hostB is active; hostA must be mounted but hidden.
        const wrapperA = screen
            .getByTestId(`ssh-terminal-session-${sessionIdA}`)
            .parentElement!.parentElement!
        const wrapperB = screen
            .getByTestId(`ssh-terminal-session-${sessionIdB}`)
            .parentElement!.parentElement!
        expect(wrapperA.style.visibility).toBe("hidden")
        expect(wrapperB.style.visibility).toBe("visible")

        // Switch the active host back to A.
        act(() => {
            useSshStore.getState().setActiveHost(hostA.id)
        })

        // Neither terminal was disposed/recreated by the switch.
        expect(xtermMock.state.terminals).toHaveLength(2)
        expect(xtermMock.state.terminals[0].dispose).not.toHaveBeenCalled()
        expect(xtermMock.state.terminals[1].dispose).not.toHaveBeenCalled()
        // No duplicate shell open for the already-open sessions.
        expect(ipcMock.sshOpenShell).toHaveBeenCalledTimes(2)

        expect(wrapperA.style.visibility).toBe("visible")
        expect(wrapperB.style.visibility).toBe("hidden")
    })
})

describe("SshPanel SFTP browser (F5)", () => {
    it("prompts to connect when the active host has no live session", async () => {
        const host = useSshStore.getState().addHost({
            name: "a",
            host: "a.example.com",
            port: 22,
            user: "root",
            authKind: "password"
        })
        act(() => {
            useSshStore.getState().setActiveHost(host.id)
            useSftpStore.getState().setActiveTab("sftp")
        })
        render(<SshPanel />)
        expect(await screen.findByText("Not connected")).toBeInTheDocument()
        // No remote listing is attempted while disconnected.
        expect(ipcMock.sftpListDir).not.toHaveBeenCalled()
    })

    it("lists the local and remote panes side by side once connected", async () => {
        ipcMock.listDir.mockResolvedValue([
            { name: "local.txt", path: "/ws/local.txt", isDir: false }
        ])
        ipcMock.sftpListDir.mockResolvedValue({
            cwd: "/home/u",
            entries: [{ name: "remote.txt", path: "/home/u/remote.txt", isDir: false, isSymlink: false, size: 5 }]
        })
        const { hostB } = await connectTwoHosts()
        act(() => {
            useWorkspaceStore.setState({ workspacePath: "/ws" })
            useSftpStore.getState().setActiveTab("sftp")
        })
        render(<SshPanel />)

        expect(await screen.findByText("remote.txt")).toBeInTheDocument()
        expect(await screen.findByText("local.txt")).toBeInTheDocument()
        // The remote pane loads the host's home directory ("" → canonical cwd).
        expect(ipcMock.sftpListDir).toHaveBeenCalledWith(
            useSshStore.getState().sessions[hostB.id]!.sessionId,
            ""
        )
    })

    it("uploads OS files dropped inside the remote pane to the remote cwd", async () => {
        ipcMock.sftpListDir.mockResolvedValue({ cwd: "/home/u", entries: [] })
        const { hostB } = await connectTwoHosts()
        const sessionId = useSshStore.getState().sessions[hostB.id]!.sessionId
        act(() => {
            useSftpStore.getState().setActiveTab("sftp")
        })
        render(<SshPanel />)

        // Wait for the remote listing (and the drag-drop listener) to be ready.
        const pane = await screen.findByTestId("sftp-remote-pane")
        await vi.waitFor(() => expect(dragMock.handler).not.toBeNull())

        // A drop whose physical position lands inside the pane's logical rect.
        pane.getBoundingClientRect = () =>
            ({ left: 100, top: 100, right: 300, bottom: 300, width: 200, height: 200, x: 100, y: 100 }) as DOMRect
        window.devicePixelRatio = 1

        await act(async () => {
            dragMock.handler!({
                payload: { type: "drop", paths: ["/a/f1.txt", "/a/f2.txt"], position: { x: 150, y: 150 } }
            })
        })

        await vi.waitFor(() => expect(ipcMock.sftpUpload).toHaveBeenCalledTimes(2))
        expect(ipcMock.sftpUpload).toHaveBeenCalledWith(sessionId, expect.any(String), "/a/f1.txt", "/home/u")
        expect(ipcMock.sftpUpload).toHaveBeenCalledWith(sessionId, expect.any(String), "/a/f2.txt", "/home/u")
    })

    it("ignores a drop that lands outside the remote pane", async () => {
        ipcMock.sftpListDir.mockResolvedValue({ cwd: "/home/u", entries: [] })
        await connectTwoHosts()
        act(() => {
            useSftpStore.getState().setActiveTab("sftp")
        })
        render(<SshPanel />)

        const pane = await screen.findByTestId("sftp-remote-pane")
        await vi.waitFor(() => expect(dragMock.handler).not.toBeNull())
        pane.getBoundingClientRect = () =>
            ({ left: 100, top: 100, right: 300, bottom: 300, width: 200, height: 200, x: 100, y: 100 }) as DOMRect
        window.devicePixelRatio = 1

        await act(async () => {
            dragMock.handler!({
                payload: { type: "drop", paths: ["/a/f1.txt"], position: { x: 10, y: 10 } }
            })
        })
        expect(ipcMock.sftpUpload).not.toHaveBeenCalled()
    })
})

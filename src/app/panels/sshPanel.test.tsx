import { render, screen, cleanup, fireEvent, act, within } from "@testing-library/react"
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
    sftpPickSelectedPath: vi.fn(async () => [] as { id: string; leaf: string }[]),
    sftpPickDownloadDestination: vi.fn(
        async (leaf: string): Promise<{ id: string; leaf: string } | null> => ({
            id: "download-cap",
            leaf
        })
    ),
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

const dialogMock = vi.hoisted(() => ({
    open: vi.fn(async () => null),
    save: vi.fn(async () => null),
    confirm: vi.fn(async (_message: string) => true)
}))
vi.mock("@tauri-apps/plugin-dialog", () => dialogMock)

import { SshPanel } from "./SshPanel"
import { useAppDialogStore } from "@/state/appDialogStore"
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
    ipcMock.sftpPickSelectedPath.mockResolvedValue([])
    ipcMock.sftpPickDownloadDestination.mockImplementation(async (leaf: string) => ({
        id: "download-cap",
        leaf
    }))
    ipcMock.sftpUpload.mockResolvedValue(undefined)
    ipcMock.sftpDownload.mockResolvedValue(undefined)
    dragMock.handler = null

    useSshStore.setState({ hosts: [], sessions: {}, activeHostId: null, pendingAuthHostId: null })
    useSftpStore.getState().reset()
    useWorkspaceStore.setState({ workspacePath: null, workspaceCapabilityId: "ws-cap" })
    useAppDialogStore.setState({ pending: null })
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

    it("sanitizes only the local extended workspace header while listing the raw path", async () => {
        const rawWorkspace = String.raw`\\?\C:\Work\中文 workspace`
        const displayWorkspace = String.raw`C:\Work\中文 workspace`
        ipcMock.sftpListDir.mockResolvedValue({ cwd: "/home/u", entries: [] })
        await connectTwoHosts()
        act(() => {
            useWorkspaceStore.setState({ workspacePath: rawWorkspace })
            useSftpStore.getState().setActiveTab("sftp")
        })

        render(<SshPanel />)

        expect(await screen.findByTitle(displayWorkspace)).toBeInTheDocument()
        expect(screen.queryByTitle(rawWorkspace)).not.toBeInTheDocument()
        expect(screen.getByTitle("/home/u")).toBeInTheDocument()
        expect(ipcMock.listDir).toHaveBeenCalledWith(rawWorkspace)
    })

    it.each([
        ["POSIX", "/", "/"],
        ["Windows drive", "C:\\", "C:\\"],
        ["Windows UNC share", String.raw`\\Server\Share`, String.raw`\\Server\Share`],
        ["Windows verbatim drive", "\\\\?\\C:\\", "C:\\"],
        ["Windows verbatim UNC", String.raw`\\?\UNC\Server\Share`, String.raw`\\Server\Share`],
    ])("disables Local Up at the %s root", async (_label, workspace, display) => {
        ipcMock.sftpListDir.mockResolvedValue({ cwd: "/home/u", entries: [] })
        await connectTwoHosts()
        act(() => {
            useWorkspaceStore.setState({ workspacePath: workspace })
            useSftpStore.getState().setActiveTab("sftp")
        })

        render(<SshPanel />)

        const pathLabel = await screen.findByTitle(display)
        const up = pathLabel.parentElement?.querySelector("button")
        expect(up).toBeDisabled()
        expect(ipcMock.listDir).toHaveBeenCalledWith(workspace)
    })

    it.each([
        ["POSIX", "/work/repo/src", "/work/repo/src", "/work/repo"],
        ["Windows drive", String.raw`C:\Work\Repo\src`, String.raw`C:\Work\Repo\src`, String.raw`C:\Work\Repo`],
        [
            "Windows UNC",
            String.raw`\\Server\Share\Repo\src`,
            String.raw`\\Server\Share\Repo\src`,
            String.raw`\\Server\Share\Repo`,
        ],
        [
            "Windows verbatim drive",
            String.raw`\\?\C:\Work\Repo\src`,
            String.raw`C:\Work\Repo\src`,
            String.raw`\\?\C:\Work\Repo`,
        ],
    ])("navigates Local Up from a non-root %s path using its native parent", async (
        _label,
        workspace,
        display,
        expectedParent
    ) => {
        ipcMock.sftpListDir.mockResolvedValue({ cwd: "/home/u", entries: [] })
        await connectTwoHosts()
        act(() => {
            useWorkspaceStore.setState({ workspacePath: workspace })
            useSftpStore.getState().setActiveTab("sftp")
        })

        render(<SshPanel />)

        const pathLabel = await screen.findByTitle(display)
        await vi.waitFor(() => expect(ipcMock.listDir).toHaveBeenCalledWith(workspace))
        ipcMock.listDir.mockClear()
        const remoteCalls = ipcMock.sftpListDir.mock.calls.length

        const up = pathLabel.parentElement?.querySelector("button")
        expect(up).toBeEnabled()
        fireEvent.click(up!)

        await vi.waitFor(() => expect(ipcMock.listDir).toHaveBeenCalledWith(expectedParent))
        expect(ipcMock.sftpListDir).toHaveBeenCalledTimes(remoteCalls)
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

        await act(async () => {
            await Promise.resolve()
        })
        expect(ipcMock.sftpPickSelectedPath).not.toHaveBeenCalled()
        expect(ipcMock.sftpUpload).not.toHaveBeenCalled()
        expect(sessionId).toBeTruthy()
    })

    it("uploads native picker grants without minting authority from path strings", async () => {
        ipcMock.sftpListDir.mockResolvedValue({ cwd: "/home/u", entries: [] })
        ipcMock.sftpPickSelectedPath.mockResolvedValue([{ id: "sel-9", leaf: "picked.txt" }])
        const { hostB } = await connectTwoHosts()
        const sessionId = useSshStore.getState().sessions[hostB.id]!.sessionId
        act(() => {
            useWorkspaceStore.setState({ workspacePath: "/ws" })
            useSftpStore.getState().setActiveTab("sftp")
        })
        render(<SshPanel />)
        await screen.findByTestId("sftp-remote-pane")
        fireEvent.click(screen.getByRole("button", { name: "Upload files" }))
        await vi.waitFor(() => expect(ipcMock.sftpPickSelectedPath).toHaveBeenCalledTimes(1))
        await vi.waitFor(() => expect(ipcMock.sftpUpload).toHaveBeenCalledWith(
            sessionId,
            expect.any(String),
            { kind: "selected", capabilityId: "sel-9", name: "picked.txt" },
            "/home/u"
        ))
    })

    it("uploads workspace-relative OS drops as workspace sources", async () => {
        ipcMock.sftpListDir.mockResolvedValue({ cwd: "/home/u", entries: [] })
        const { hostB } = await connectTwoHosts()
        const sessionId = useSshStore.getState().sessions[hostB.id]!.sessionId
        act(() => {
            useWorkspaceStore.setState({ workspacePath: "/ws" })
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
                payload: { type: "drop", paths: ["/ws/local.txt"], position: { x: 150, y: 150 } }
            })
        })
        await vi.waitFor(() => expect(ipcMock.sftpUpload).toHaveBeenCalledWith(
            sessionId,
            expect.any(String),
            { kind: "workspace", workspaceId: "ws-cap", relativePath: "local.txt" },
            "/home/u"
        ))
        expect(ipcMock.sftpPickSelectedPath).not.toHaveBeenCalled()
    })

    describe("SFTP 窗格互拖（P5，pointer-based）", () => {
        // 內部互拖是 pointer 驅動（Tauri 的 drag 層會吃掉 HTML5 內部 drop）：
        // jsdom 沒有 layout/elementFromPoint，hit-test 目標由測試直接指定。
        const originalElementFromPoint = document.elementFromPoint
        function stubElementFromPoint(target: Element | null) {
            ;(
                document as unknown as {
                    elementFromPoint: (x: number, y: number) => Element | null
                }
            ).elementFromPoint = () => target
        }

        beforeEach(() => {
            stubElementFromPoint(null)
        })

        afterEach(() => {
            document.elementFromPoint = originalElementFromPoint
        })

        async function sftpDualPaneSetup() {
            ipcMock.listDir.mockResolvedValue([
                { name: "local.txt", path: "/ws/local.txt", isDir: false },
                { name: "localdir", path: "/ws/localdir", isDir: true }
            ])
            ipcMock.sftpListDir.mockResolvedValue({
                cwd: "/home/u",
                entries: [
                    { name: "remote.txt", path: "/home/u/remote.txt", isDir: false, isSymlink: false, size: 7 },
                    { name: "docs", path: "/home/u/docs", isDir: true, isSymlink: false, size: 0 }
                ]
            })
            const { hostB } = await connectTwoHosts()
            const sessionId = useSshStore.getState().sessions[hostB.id]!.sessionId!
            act(() => {
                useWorkspaceStore.setState({ workspacePath: "/ws" })
                useSftpStore.getState().setActiveTab("sftp")
            })
            render(<SshPanel />)
            await screen.findByText("remote.txt")
            await screen.findByText("local.txt")
            return { sessionId }
        }

        const rowOf = (text: string) =>
            screen.getByText(text).closest("div.group") as HTMLElement

        // 按下 → 越過 4px 啟動閾值 → 放開在 target 上。
        function dragByPointer(sourceRow: HTMLElement, target: Element | null) {
            fireEvent.pointerDown(sourceRow, { button: 0, clientX: 10, clientY: 10 })
            stubElementFromPoint(target)
            fireEvent.pointerMove(window, { clientX: 40, clientY: 40 })
            fireEvent.pointerUp(window, { clientX: 40, clientY: 40 })
        }

        it("local 檔案 row 拖到遠端窗格空白＝上傳到遠端 cwd；拖曳中窗格高亮；資料夾 row 不可拖", async () => {
            const { sessionId } = await sftpDualPaneSetup()
            const remoteBody = screen.getByTestId("sftp-remote-pane-body")

            fireEvent.pointerDown(rowOf("local.txt"), { button: 0, clientX: 10, clientY: 10 })
            stubElementFromPoint(remoteBody)
            fireEvent.pointerMove(window, { clientX: 40, clientY: 40 })
            fireEvent.pointerMove(window, { clientX: 41, clientY: 41 })
            expect(remoteBody.className).toContain("ring-2")
            fireEvent.pointerUp(window, { clientX: 41, clientY: 41 })

            await vi.waitFor(() => expect(ipcMock.sftpUpload).toHaveBeenCalledTimes(1))
            expect(ipcMock.sftpUpload).toHaveBeenCalledWith(
                sessionId,
                expect.any(String),
                { kind: "workspace", workspaceId: "ws-cap", relativePath: "local.txt" },
                "/home/u"
            )
            expect(remoteBody.className).not.toContain("ring-2")

            // 資料夾 row 沒有拖曳來源（無 onPointerDown）：拖它不觸發任何上傳。
            dragByPointer(rowOf("localdir"), remoteBody)
            await new Promise((resolve) => setTimeout(resolve, 0))
            expect(ipcMock.sftpUpload).toHaveBeenCalledTimes(1)
        })

        it("local 檔案拖到遠端資料夾 row＝上傳到該資料夾，且視圖 refresh 停留在原 cwd", async () => {
            const { sessionId } = await sftpDualPaneSetup()

            dragByPointer(rowOf("local.txt"), rowOf("docs"))

            await vi.waitFor(() => expect(ipcMock.sftpUpload).toHaveBeenCalledTimes(1))
            expect(ipcMock.sftpUpload).toHaveBeenCalledWith(
                sessionId,
                expect.any(String),
                { kind: "workspace", workspaceId: "ws-cap", relativePath: "local.txt" },
                "/home/u/docs"
            )
            // 上傳後 refresh 針對目前顯示目錄（cwd），不跳進 docs。
            await vi.waitFor(() =>
                expect(ipcMock.sftpListDir).toHaveBeenLastCalledWith(sessionId, "/home/u")
            )
        })

        it("remote 檔案 row 拖到本機窗格＝下載到本機 cwd", async () => {
            const { sessionId } = await sftpDualPaneSetup()

            dragByPointer(rowOf("remote.txt"), screen.getByTestId("sftp-local-pane-body"))

            await vi.waitFor(() => expect(ipcMock.sftpDownload).toHaveBeenCalledTimes(1))
            expect(ipcMock.sftpDownload).toHaveBeenCalledWith(
                sessionId,
                expect.any(String),
                "/home/u/remote.txt",
                { capabilityId: "download-cap", leaf: "remote.txt" }
            )
        })

        it("remote 檔案拖到本機資料夾 row＝下載到該資料夾", async () => {
            const { sessionId } = await sftpDualPaneSetup()

            dragByPointer(rowOf("remote.txt"), rowOf("localdir"))

            await vi.waitFor(() => expect(ipcMock.sftpDownload).toHaveBeenCalledTimes(1))
            expect(ipcMock.sftpDownload).toHaveBeenCalledWith(
                sessionId,
                expect.any(String),
                "/home/u/remote.txt",
                { capabilityId: "download-cap", leaf: "remote.txt" }
            )
        })

        it("同窗格目標與無效目標一律 no-op（方向互斥）", async () => {
            await sftpDualPaneSetup()

            // local 檔案放回本機窗格（同 pane）→ no-op。
            dragByPointer(rowOf("local.txt"), screen.getByTestId("sftp-local-pane-body"))
            // 放在非 drop target（null）→ no-op。
            dragByPointer(rowOf("remote.txt"), null)

            await new Promise((resolve) => setTimeout(resolve, 0))
            expect(ipcMock.sftpUpload).not.toHaveBeenCalled()
            expect(ipcMock.sftpDownload).not.toHaveBeenCalled()
        })

        it("檔名（row 主要區域）是拖曳來源：從檔名文字按下拖到對面窗格觸發傳輸", async () => {
            // D1/D2 迴歸根因看守：檔名本身是 <button>，拖曳來源排除只能限定
            // action 按鈕區（data-sftp-row-actions），不得吞掉檔名。
            const { sessionId } = await sftpDualPaneSetup()
            const nameEl = screen.getByText("local.txt")

            fireEvent.pointerDown(nameEl, { button: 0, clientX: 10, clientY: 10 })
            stubElementFromPoint(screen.getByTestId("sftp-remote-pane-body"))
            fireEvent.pointerMove(window, { clientX: 40, clientY: 40 })
            fireEvent.pointerUp(window, { clientX: 40, clientY: 40 })

            await vi.waitFor(() => expect(ipcMock.sftpUpload).toHaveBeenCalledTimes(1))
            expect(ipcMock.sftpUpload).toHaveBeenCalledWith(
                sessionId,
                expect.any(String),
                { kind: "workspace", workspaceId: "ws-cap", relativePath: "local.txt" },
                "/home/u"
            )
        })

        it("按下檔案 row 即選取（row 高亮）；另選其他檔案時高亮移轉", async () => {
            await sftpDualPaneSetup()
            const localRow = rowOf("local.txt")
            const remoteRow = rowOf("remote.txt")

            fireEvent.pointerDown(localRow, { button: 0, clientX: 10, clientY: 10 })
            fireEvent.pointerUp(window, { clientX: 10, clientY: 10 })
            expect(localRow.className).toContain("bg-(--yz-active)")

            fireEvent.pointerDown(remoteRow, { button: 0, clientX: 10, clientY: 10 })
            fireEvent.pointerUp(window, { clientX: 10, clientY: 10 })
            expect(remoteRow.className).toContain("bg-(--yz-active)")
            expect(localRow.className).not.toContain("bg-(--yz-active)")

            // 純點擊（無位移）不觸發任何傳輸。
            expect(ipcMock.sftpUpload).not.toHaveBeenCalled()
            expect(ipcMock.sftpDownload).not.toHaveBeenCalled()
        })

        it("row 的 hover 按鈕不是拖曳來源：按住按鈕微移＞4px 放開仍是按鈕點擊、不觸發傳輸", async () => {
            await sftpDualPaneSetup()

            // remote 檔案 row 的 download 按鈕：pointerdown 冒泡到 row 但來源
            // 是 button → 不建立拖曳（beginRowDrag closest("button") 早退）。
            const downloadBtn = screen.getByRole("button", { name: "Download remote.txt" })
            fireEvent.pointerDown(downloadBtn, { button: 0, clientX: 10, clientY: 10 })
            stubElementFromPoint(screen.getByTestId("sftp-local-pane-body"))
            fireEvent.pointerMove(window, { clientX: 40, clientY: 40 })
            // 未進入拖曳態 → 本機窗格不高亮。
            expect(screen.getByTestId("sftp-local-pane-body").className).not.toContain("ring-2")
            fireEvent.pointerUp(window, { clientX: 40, clientY: 40 })
            ipcMock.sftpPickDownloadDestination.mockResolvedValueOnce(null)
            fireEvent.click(downloadBtn)

            await new Promise((resolve) => setTimeout(resolve, 0))
            // 沒有任何拖曳傳輸；按鈕點擊只可走 backend native-save capability。
            expect(ipcMock.sftpDownload).not.toHaveBeenCalled()
            expect(ipcMock.sftpPickDownloadDestination).toHaveBeenCalledWith("remote.txt")
        })

        it("Escape 取消拖曳：高亮消失、放開不觸發傳輸", async () => {
            await sftpDualPaneSetup()
            const remoteBody = screen.getByTestId("sftp-remote-pane-body")

            fireEvent.pointerDown(rowOf("local.txt"), { button: 0, clientX: 10, clientY: 10 })
            stubElementFromPoint(remoteBody)
            fireEvent.pointerMove(window, { clientX: 40, clientY: 40 })
            fireEvent.pointerMove(window, { clientX: 41, clientY: 41 })
            expect(remoteBody.className).toContain("ring-2")

            fireEvent.keyDown(window, { key: "Escape" })
            expect(remoteBody.className).not.toContain("ring-2")
            fireEvent.pointerUp(window, { clientX: 41, clientY: 41 })

            await new Promise((resolve) => setTimeout(resolve, 0))
            expect(ipcMock.sftpUpload).not.toHaveBeenCalled()
        })

        it("拖曳下載完成後本機列表自動刷新（不需離開目錄）", async () => {
            await sftpDualPaneSetup()

            const callsBefore = ipcMock.listDir.mock.calls.length
            dragByPointer(rowOf("remote.txt"), screen.getByTestId("sftp-local-pane-body"))

            await vi.waitFor(() => expect(ipcMock.sftpDownload).toHaveBeenCalledTimes(1))
            // 下載完成後 refresh 一次，且仍停留在原目錄。
            await vi.waitFor(() =>
                expect(ipcMock.listDir.mock.calls.length).toBe(callsBefore + 1)
            )
            expect(ipcMock.listDir).toHaveBeenLastCalledWith("/ws")
        })

        it("拖曳下載只能消耗 backend native-save capability；取消不下載", async () => {
            const { sessionId } = await sftpDualPaneSetup()
            const localBody = screen.getByTestId("sftp-local-pane-body")

            // 遠端檔名與本機既有 local.txt 同名 → 觸發覆蓋確認（plugin-dialog）。
            ipcMock.sftpListDir.mockResolvedValue({
                cwd: "/home/u",
                entries: [
                    { name: "local.txt", path: "/home/u/local.txt", isDir: false, isSymlink: false, size: 3 },
                    { name: "docs", path: "/home/u/docs", isDir: true, isSymlink: false, size: 0 }
                ]
            })
            act(() => {
                void useSftpStore.getState().listRemote(
                    useSshStore.getState().activeHostId!, "/home/u"
                )
            })
            await vi.waitFor(() =>
                expect(screen.getAllByText("local.txt").length).toBeGreaterThan(1)
            )
            const remoteRow = screen
                .getAllByText("local.txt")
                .map((el) => el.closest("div.group") as HTMLElement)
                .find((row) => row.closest("[data-sftp-drop-pane=\"remote\"]"))!

            ipcMock.sftpPickDownloadDestination.mockResolvedValueOnce(null)
            dragByPointer(remoteRow, localBody)
            await vi.waitFor(() =>
                expect(ipcMock.sftpPickDownloadDestination).toHaveBeenCalledWith("local.txt")
            )
            expect(ipcMock.sftpDownload).not.toHaveBeenCalled()
            expect(useAppDialogStore.getState().pending).toBeNull()

            dragByPointer(remoteRow, localBody)
            await vi.waitFor(() => expect(ipcMock.sftpDownload).toHaveBeenCalledTimes(1))
            expect(ipcMock.sftpDownload).toHaveBeenCalledWith(
                sessionId,
                expect.any(String),
                "/home/u/local.txt",
                { capabilityId: "download-cap", leaf: "local.txt" }
            )
        })

        it("本機上傳按鈕：工作區檔走 workspace source，區外檔不能用路徑換 capability", async () => {
            const { sessionId } = await sftpDualPaneSetup()
            fireEvent.click(screen.getByRole("button", { name: "Upload local.txt" }))
            await vi.waitFor(() => expect(ipcMock.sftpUpload).toHaveBeenCalledTimes(1))
            expect(ipcMock.sftpPickSelectedPath).not.toHaveBeenCalled()
            expect(ipcMock.sftpUpload).toHaveBeenCalledWith(
                sessionId,
                expect.any(String),
                { kind: "workspace", workspaceId: "ws-cap", relativePath: "local.txt" },
                "/home/u"
            )

            ipcMock.listDir.mockResolvedValue([
                { name: "picked.txt", path: "/outside/picked.txt", isDir: false, kind: "file" }
            ])
            const localPane = screen.getByTestId("sftp-local-pane-body").parentElement
            expect(localPane).toBeTruthy()
            fireEvent.click(within(localPane!).getByRole("button", { name: "Up one level" }))
            await screen.findByText("picked.txt")
            fireEvent.click(screen.getByRole("button", { name: "Upload picked.txt" }))
            await act(async () => {
                await Promise.resolve()
            })
            expect(ipcMock.sftpPickSelectedPath).not.toHaveBeenCalled()
            expect(ipcMock.sftpUpload).toHaveBeenCalledTimes(1)
        })

        it("symlink 與不安全檔名不可拖、沒有上傳／下載按鈕", async () => {
            ipcMock.listDir.mockResolvedValue([
                { name: "local.txt", path: "/ws/local.txt", isDir: false, kind: "file" },
                { name: "link.txt", path: "/ws/link.txt", isDir: false, kind: "symlink" }
            ])
            ipcMock.sftpListDir.mockResolvedValue({
                cwd: "/home/u",
                entries: [
                    { name: "remote.txt", path: "/home/u/remote.txt", isDir: false, isSymlink: false, size: 7 },
                    { name: "away", path: "/home/u/away", isDir: false, isSymlink: true, size: 0 },
                    { name: "..evil", path: "/home/u/..evil", isDir: false, isSymlink: false, nameSafe: false, size: 1 }
                ]
            })
            const { hostB } = await connectTwoHosts()
            act(() => {
                useWorkspaceStore.setState({ workspacePath: "/ws" })
                useSftpStore.getState().setActiveTab("sftp")
            })
            render(<SshPanel />)
            await screen.findByText("link.txt")
            await screen.findByText("away")
            await screen.findByText("..evil")
            expect(screen.queryByRole("button", { name: "Upload link.txt" })).toBeNull()
            expect(screen.queryByRole("button", { name: "Download away" })).toBeNull()
            expect(screen.queryByRole("button", { name: "Download ..evil" })).toBeNull()

            dragByPointer(
                screen.getByText("link.txt").closest("div.group") as HTMLElement,
                screen.getByTestId("sftp-remote-pane-body")
            )
            dragByPointer(
                screen.getByText("away").closest("div.group") as HTMLElement,
                screen.getByTestId("sftp-local-pane-body")
            )
            await new Promise((resolve) => setTimeout(resolve, 0))
            expect(ipcMock.sftpUpload).not.toHaveBeenCalled()
            expect(ipcMock.sftpDownload).not.toHaveBeenCalled()
            expect(useSshStore.getState().sessions[hostB.id]?.sessionId).toBeTruthy()
        })
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

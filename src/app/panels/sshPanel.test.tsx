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

const dialogMock = vi.hoisted(() => ({
    open: vi.fn(async () => null),
    save: vi.fn(async () => null),
    confirm: vi.fn(async (_message: string) => true)
}))
vi.mock("@tauri-apps/plugin-dialog", () => dialogMock)

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
                sessionId, expect.any(String), "/ws/local.txt", "/home/u"
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
                sessionId, expect.any(String), "/ws/local.txt", "/home/u/docs"
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
                sessionId, expect.any(String), "/home/u/remote.txt", "/ws/remote.txt"
            )
        })

        it("remote 檔案拖到本機資料夾 row＝下載到該資料夾", async () => {
            const { sessionId } = await sftpDualPaneSetup()

            dragByPointer(rowOf("remote.txt"), rowOf("localdir"))

            await vi.waitFor(() => expect(ipcMock.sftpDownload).toHaveBeenCalledTimes(1))
            expect(ipcMock.sftpDownload).toHaveBeenCalledWith(
                sessionId, expect.any(String), "/home/u/remote.txt", "/ws/localdir/remote.txt"
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
                sessionId, expect.any(String), "/ws/local.txt", "/home/u"
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
            fireEvent.click(downloadBtn)

            await new Promise((resolve) => setTimeout(resolve, 0))
            // 沒有任何拖曳傳輸；按鈕點擊走 saveDialog 路徑（cancel → 無下載呼叫）。
            expect(ipcMock.sftpDownload).not.toHaveBeenCalled()
            expect(dialogMock.save).toHaveBeenCalledTimes(1)
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
            // 覆蓋檢查 list 一次＋下載完成 refresh 一次，且都停留在原目錄。
            await vi.waitFor(() =>
                expect(ipcMock.listDir.mock.calls.length).toBe(callsBefore + 2)
            )
            expect(ipcMock.listDir).toHaveBeenLastCalledWith("/ws")
        })

        it("拖曳下載遇本機同名檔：confirm 取消 → 不下載；確認 → 覆蓋下載", async () => {
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

            dialogMock.confirm.mockResolvedValueOnce(false)
            dragByPointer(remoteRow, localBody)
            await vi.waitFor(() => expect(dialogMock.confirm).toHaveBeenCalledTimes(1))
            expect(String(dialogMock.confirm.mock.calls[0][0])).toContain("local.txt")
            expect(ipcMock.sftpDownload).not.toHaveBeenCalled()

            dialogMock.confirm.mockResolvedValueOnce(true)
            dragByPointer(remoteRow, localBody)
            await vi.waitFor(() => expect(ipcMock.sftpDownload).toHaveBeenCalledTimes(1))
            expect(ipcMock.sftpDownload).toHaveBeenCalledWith(
                sessionId, expect.any(String), "/home/u/local.txt", "/ws/local.txt"
            )
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

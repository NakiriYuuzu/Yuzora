import { beforeEach, describe, expect, it, vi } from "vitest"
import { clearMocks, mockIPC } from "@tauri-apps/api/mocks"
import { EditorState } from "@codemirror/state"
import { EditorView } from "@codemirror/view"
import type { MouseEvent as ReactMouseEvent } from "react"

import {
    contextMenuHandler,
    runContextMenuAction,
    suppressContextMenu,
    useContextMenuStore
} from "@/state/contextMenuStore"
import { useAgentStore } from "@/state/agentStore"
import { useDiffModalStore } from "@/state/diffModalStore"
import { useGitStore } from "@/state/gitStore"
import { usePreviewStore } from "@/state/previewStore"
import { useSftpStore } from "@/state/sftpStore"
import { useSshStore } from "@/state/sshStore"
import { useTerminalStore, terminalInitialState } from "@/state/terminalStore"
import { useUiStore, uiInitialState } from "@/state/uiStore"
import { useWorkspaceStore } from "@/state/workspaceStore"
import { registerView, unregisterView } from "@/editor/viewRegistry"
import type { GitStatus } from "@/lib/types"

// contextMenuStore's cmFormatDoc dispatch calls the real @codemirror/lsp-client
// formatDocument Command against the active view — mocked here (not the whole
// LSP stack) so the wiring test only asserts "dispatch reached the command",
// without needing a live language server. Declared before the mock factory so
// the closure captures this exact spy (same pattern as lspExtensions.test.ts).
// vi.mock is hoisted above every import above regardless of this textual
// position, so contextMenuStore's own static import of formatDocument still
// resolves to this mock.
const formatDocument = vi.fn()
vi.mock("@codemirror/lsp-client", () => ({
    formatDocument: (view: unknown) => formatDocument(view)
}))

// Registered under the view registry so runContextMenuAction("editor", …) can
// resolve it as "the active editor" the same way EditorPane does in prod.
const EDIT_PATH = "/w/edit.ts"

function makeEditorView(doc: string, selection?: { anchor: number; head?: number }): EditorView {
    return new EditorView({ state: EditorState.create({ doc, selection }) })
}

function makeStatus(over: Partial<GitStatus> = {}): GitStatus {
    return {
        branch: "main",
        headOid: "0".repeat(40),
        detached: false,
        upstream: null,
        ahead: 0,
        behind: 0,
        staged: [],
        unstaged: [],
        untracked: [],
        conflicted: [],
        inProgress: null,
        ...over
    }
}

beforeEach(() => {
    clearMocks()
    mockIPC((cmd) => (cmd === "log_event" ? null : undefined))
    vi.clearAllMocks()
    useContextMenuStore.setState({ kind: null, x: 0, y: 0, payload: {} })
    useDiffModalStore.setState({ open: false, source: null, activeIndex: 0, mode: "unified" })
    useUiStore.setState(uiInitialState)
    useWorkspaceStore.setState({
        workspacePath: null,
        groups: [{ tabs: [], activePath: null }],
        activeGroupIndex: 0
    })
    useGitStore.setState({ status: null, environment: null, busy: null, lastError: null, consoleLog: [] })
    useTerminalStore.setState(terminalInitialState)
    useAgentStore.setState({
        sessions: new Map(),
        pendingPermissions: new Map(),
        activeSessionId: null,
        connectionState: "idle",
        connectionError: null,
        connection: null,
        authRequired: null
    })
})

describe("contextMenuStore", () => {
    it("open 記錄 kind、座標與 payload；close 清掉 kind", () => {
        useContextMenuStore.getState().open("tab", 120, 240, { path: "/a.md", groupIndex: 0 })
        expect(useContextMenuStore.getState()).toMatchObject({
            kind: "tab",
            x: 120,
            y: 240,
            payload: { path: "/a.md", groupIndex: 0 }
        })
        useContextMenuStore.getState().close()
        expect(useContextMenuStore.getState().kind).toBeNull()
    })

    it("contextMenuHandler 阻止原生選單、停止冒泡並開啟對應選單", () => {
        const event = {
            preventDefault: vi.fn(),
            stopPropagation: vi.fn(),
            clientX: 33,
            clientY: 44
        } as unknown as ReactMouseEvent
        contextMenuHandler("rail")(event)
        expect(event.preventDefault).toHaveBeenCalled()
        expect(event.stopPropagation).toHaveBeenCalled()
        expect(useContextMenuStore.getState()).toMatchObject({ kind: "rail", x: 33, y: 44 })
    })

    it("suppressContextMenu 只吃掉事件，不開選單", () => {
        const event = {
            preventDefault: vi.fn(),
            stopPropagation: vi.fn()
        } as unknown as ReactMouseEvent
        suppressContextMenu(event)
        expect(event.preventDefault).toHaveBeenCalled()
        expect(event.stopPropagation).toHaveBeenCalled()
        expect(useContextMenuStore.getState().kind).toBeNull()
    })

    it("runContextMenuAction 關閉選單並記 user-action log（UI-only stub）", async () => {
        const logged: unknown[] = []
        mockIPC((cmd, args) => {
            if (cmd === "log_event") {
                logged.push(args)
                return null
            }
            return undefined
        })
        useContextMenuStore.getState().open("tab", 0, 0, { path: "/a.md" })
        runContextMenuAction("tab", "cmCloseTab", { path: "/a.md" })
        expect(useContextMenuStore.getState().kind).toBeNull()
        await Promise.resolve()
        expect(logged).toHaveLength(1)
        const entry = (logged[0] as { event: { event: string; message: string; metadata: { path?: string } } }).event
        expect(entry.event).toBe("context_menu_action")
        expect(entry.message).toBe("tab:cmCloseTab")
        expect(entry.metadata.path).toBe("/a.md")
    })

    it("cmCompareHead 把絕對 activePath 相對化後對『在變更清單中的檔案』開 Diff modal", () => {
        // Editor tabs open with ABSOLUTE paths (FileTree passes node.path); git
        // status paths are repo-relative. cmCompareHead must strip the repo root
        // before matching — otherwise this entry is a永久 no-op in production.
        useWorkspaceStore.setState({
            groups: [{ tabs: [], activePath: "/w/src/git.rs" }],
            activeGroupIndex: 0
        })
        useGitStore.setState({
            environment: { status: "ready", root: "/w", version: "2.50" },
            status: makeStatus({ unstaged: [{ path: "src/git.rs", origPath: null, status: "M" }] })
        })
        runContextMenuAction("editor", "cmCompareHead", {})
        expect(useUiStore.getState().mode).toBe("git")
        const s = useDiffModalStore.getState()
        expect(s.open).toBe(true)
        expect(s.source?.type).toBe("worktree")
        // Active file is auto-selected in the file list (matched on the
        // repo-relative path).
        if (s.source?.type === "worktree") {
            expect(s.source.files.some((f) => f.path === "src/git.rs")).toBe(true)
            expect(s.activeIndex).toBe(s.source.files.findIndex((f) => f.path === "src/git.rs"))
        }
    })

    it("cmCompareHead 對不在變更清單中的檔案是 no-op", () => {
        useWorkspaceStore.setState({
            groups: [{ tabs: [], activePath: "/w/src/clean.rs" }],
            activeGroupIndex: 0
        })
        useGitStore.setState({
            environment: { status: "ready", root: "/w", version: "2.50" },
            status: makeStatus({ unstaged: [{ path: "src/git.rs", origPath: null, status: "M" }] })
        })
        runContextMenuAction("editor", "cmCompareHead", {})
        expect(useDiffModalStore.getState().open).toBe(false)
        expect(useUiStore.getState().mode).toBe(uiInitialState.mode)
    })

    it("cmCompareHead 無 active 檔時 no-op", () => {
        useGitStore.setState({
            environment: { status: "ready", root: "/w", version: "2.50" },
            status: makeStatus({ unstaged: [{ path: "src/git.rs", origPath: null, status: "M" }] })
        })
        runContextMenuAction("editor", "cmCompareHead", {})
        expect(useDiffModalStore.getState().open).toBe(false)
    })

    it("cmCompareHead 在 repo 未 ready 或 active 檔在 root 之外時 no-op", () => {
        // 非 ready environment：即使 activePath 看似匹配也不觸發（無 root 可相對化）。
        useWorkspaceStore.setState({
            groups: [{ tabs: [], activePath: "/w/src/git.rs" }],
            activeGroupIndex: 0
        })
        useGitStore.setState({
            environment: { status: "notARepo" },
            status: makeStatus({ unstaged: [{ path: "src/git.rs", origPath: null, status: "M" }] })
        })
        runContextMenuAction("editor", "cmCompareHead", {})
        expect(useDiffModalStore.getState().open).toBe(false)

        // active 檔在 repo root 之外：無法相對化 → no-op。
        useWorkspaceStore.setState({
            groups: [{ tabs: [], activePath: "/other/src/git.rs" }],
            activeGroupIndex: 0
        })
        useGitStore.setState({
            environment: { status: "ready", root: "/w", version: "2.50" },
            status: makeStatus({ unstaged: [{ path: "src/git.rs", origPath: null, status: "M" }] })
        })
        runContextMenuAction("editor", "cmCompareHead", {})
        expect(useDiffModalStore.getState().open).toBe(false)
    })
})

// PROB-5 前波：純前端可完成的 action 真的接到目標 API（而非只 close+log）。
describe("runContextMenuAction — 前端接線 (PROB-5)", () => {
    it("cmSettings 開啟 Settings dialog（general／rail 共用）", () => {
        runContextMenuAction("general", "cmSettings", {})
        expect(useUiStore.getState().settingsOpen).toBe(true)
    })

    it("cmHideSidebar bump sidebarToggleRequest（AppShell 據此切換 navCollapsed，見該 store）", () => {
        const before = useUiStore.getState().sidebarToggleRequest
        runContextMenuAction("rail", "cmHideSidebar", {})
        expect(useUiStore.getState().sidebarToggleRequest).toBe(before + 1)
    })

    it("cmCmdPalette（general／editor 共用）bump paletteOpenRequest", () => {
        const before = useUiStore.getState().paletteOpenRequest
        runContextMenuAction("general", "cmCmdPalette", {})
        expect(useUiStore.getState().paletteOpenRequest).toBe(before + 1)
        runContextMenuAction("editor", "cmCmdPalette", {})
        expect(useUiStore.getState().paletteOpenRequest).toBe(before + 2)
    })

    it("tab: cmCloseTab 直接關閉非 dirty tab（不彈 confirm）", async () => {
        let dialogCalled = false
        mockIPC((cmd) => {
            if (cmd === "plugin:dialog|message") {
                dialogCalled = true
                return "Ok"
            }
            return cmd === "log_event" ? null : undefined
        })
        useWorkspaceStore.setState({
            workspacePath: "/w",
            activeGroupIndex: 0,
            groups: [
                {
                    activePath: "/w/a.ts",
                    tabs: [
                        { path: "/w/a.ts", name: "a.ts", dirty: false, externallyModified: false },
                        { path: "/w/b.ts", name: "b.ts", dirty: false, externallyModified: false }
                    ]
                }
            ]
        })
        runContextMenuAction("tab", "cmCloseTab", { path: "/w/b.ts", groupIndex: 0 })
        await vi.waitFor(() => {
            expect(useWorkspaceStore.getState().groups[0].tabs.map((t) => t.path)).toEqual(["/w/a.ts"])
        })
        expect(dialogCalled).toBe(false)
    })

    it("tab: cmCloseTab 對 dirty tab 先 confirm — 取消則保留、確定才真的關閉", async () => {
        useWorkspaceStore.setState({
            workspacePath: "/w",
            activeGroupIndex: 0,
            groups: [
                {
                    activePath: "/w/b.ts",
                    tabs: [
                        { path: "/w/a.ts", name: "a.ts", dirty: false, externallyModified: false },
                        { path: "/w/b.ts", name: "b.ts", dirty: true, externallyModified: false }
                    ]
                }
            ]
        })
        mockIPC((cmd) => {
            if (cmd === "plugin:dialog|message") return "Cancel"
            return cmd === "log_event" ? null : undefined
        })
        runContextMenuAction("tab", "cmCloseTab", { path: "/w/b.ts", groupIndex: 0 })
        await vi.waitFor(() => {
            expect(useWorkspaceStore.getState().groups[0].tabs.some((t) => t.path === "/w/b.ts")).toBe(true)
        })

        mockIPC((cmd) => {
            if (cmd === "plugin:dialog|message") return "Ok"
            return cmd === "log_event" ? null : undefined
        })
        runContextMenuAction("tab", "cmCloseTab", { path: "/w/b.ts", groupIndex: 0 })
        await vi.waitFor(() => {
            expect(useWorkspaceStore.getState().groups[0].tabs.some((t) => t.path === "/w/b.ts")).toBe(false)
        })
    })

    it("tab: cmCloseOthers 關閉其餘 tabs、保留指定 path", async () => {
        useWorkspaceStore.setState({
            workspacePath: "/w",
            activeGroupIndex: 0,
            groups: [
                {
                    activePath: "/w/a.ts",
                    tabs: [
                        { path: "/w/a.ts", name: "a.ts", dirty: false, externallyModified: false },
                        { path: "/w/b.ts", name: "b.ts", dirty: false, externallyModified: false },
                        { path: "/w/c.ts", name: "c.ts", dirty: false, externallyModified: false }
                    ]
                }
            ]
        })
        runContextMenuAction("tab", "cmCloseOthers", { path: "/w/b.ts", groupIndex: 0 })
        await vi.waitFor(() => {
            expect(useWorkspaceStore.getState().groups[0].tabs.map((t) => t.path)).toEqual(["/w/b.ts"])
        })
        expect(useWorkspaceStore.getState().groups[0].activePath).toBe("/w/b.ts")
    })

    it("tab: cmCloseAll 清空該 group 的所有 tabs", async () => {
        useWorkspaceStore.setState({
            workspacePath: "/w",
            activeGroupIndex: 0,
            groups: [
                {
                    activePath: "/w/a.ts",
                    tabs: [
                        { path: "/w/a.ts", name: "a.ts", dirty: false, externallyModified: false },
                        { path: "/w/b.ts", name: "b.ts", dirty: false, externallyModified: false }
                    ]
                }
            ]
        })
        runContextMenuAction("tab", "cmCloseAll", { groupIndex: 0 })
        await vi.waitFor(() => {
            expect(useWorkspaceStore.getState().groups[0].tabs).toEqual([])
        })
        expect(useWorkspaceStore.getState().groups[0].activePath).toBeNull()
    })

    it("tab/file: cmCopyRel 寫入相對 workspacePath 的路徑；workspace 之外退回絕對路徑", async () => {
        const calls: Array<{ cmd: string; args: unknown }> = []
        mockIPC((cmd, args) => {
            calls.push({ cmd, args })
            return cmd === "log_event" ? null : undefined
        })
        useWorkspaceStore.setState({ workspacePath: "/w" })

        runContextMenuAction("tab", "cmCopyRel", { path: "/w/src/a.ts", groupIndex: 0 })
        await vi.waitFor(() => {
            expect(calls.some((c) => c.cmd === "plugin:clipboard-manager|write_text")).toBe(true)
        })
        expect(
            (calls.find((c) => c.cmd === "plugin:clipboard-manager|write_text")?.args as { text: string }).text
        ).toBe("src/a.ts")

        calls.length = 0
        runContextMenuAction("file", "cmCopyRel", { path: "/other/a.ts" })
        await vi.waitFor(() => {
            expect(calls.some((c) => c.cmd === "plugin:clipboard-manager|write_text")).toBe(true)
        })
        expect(
            (calls.find((c) => c.cmd === "plugin:clipboard-manager|write_text")?.args as { text: string }).text
        ).toBe("/other/a.ts")
    })

    it("tab/file: cmSplit／cmOpenSplit 都呼叫既有的 splitRight（groups/split 機制）", () => {
        useWorkspaceStore.setState({ groups: [{ tabs: [], activePath: null }], activeGroupIndex: 0 })
        runContextMenuAction("tab", "cmSplit", { path: "/w/a.ts", groupIndex: 0 })
        expect(useWorkspaceStore.getState().groups).toHaveLength(2)

        useWorkspaceStore.setState({ groups: [{ tabs: [], activePath: null }], activeGroupIndex: 0 })
        runContextMenuAction("file", "cmOpenSplit", { path: "/w/a.ts" })
        expect(useWorkspaceStore.getState().groups).toHaveLength(2)
    })

    it("file: cmOpen 開啟該檔案分頁", () => {
        useWorkspaceStore.setState({ groups: [{ tabs: [], activePath: null }], activeGroupIndex: 0 })
        runContextMenuAction("file", "cmOpen", { path: "/w/a.ts" })
        const group = useWorkspaceStore.getState().groups[0]
        expect(group.activePath).toBe("/w/a.ts")
        expect(group.tabs.some((t) => t.path === "/w/a.ts")).toBe(true)
    })

    it("explorer: cmCopyPath 複製目前 workspacePath", async () => {
        const calls: Array<{ cmd: string; args: unknown }> = []
        mockIPC((cmd, args) => {
            calls.push({ cmd, args })
            return cmd === "log_event" ? null : undefined
        })
        useWorkspaceStore.setState({ workspacePath: "/w/project" })
        runContextMenuAction("explorer", "cmCopyPath", {})
        await vi.waitFor(() => {
            expect(calls.some((c) => c.cmd === "plugin:clipboard-manager|write_text")).toBe(true)
        })
        expect(
            (calls.find((c) => c.cmd === "plugin:clipboard-manager|write_text")?.args as { text: string }).text
        ).toBe("/w/project")
    })

    it("agent: cmCopyPath 沒有可用的 payload/store 對映，維持 UI-only（deferred，不誤觸 explorer 的邏輯）", async () => {
        const calls: string[] = []
        mockIPC((cmd) => {
            calls.push(cmd)
            return cmd === "log_event" ? null : undefined
        })
        useWorkspaceStore.setState({ workspacePath: "/w/project" })
        runContextMenuAction("agent", "cmCopyPath", {})
        await new Promise((resolve) => setTimeout(resolve, 0))
        expect(calls).not.toContain("plugin:clipboard-manager|write_text")
    })

    it("editor: cmCopy 複製選取文字到 clipboard", async () => {
        const calls: Array<{ cmd: string; args: unknown }> = []
        mockIPC((cmd, args) => {
            calls.push({ cmd, args })
            return cmd === "log_event" ? null : undefined
        })
        const view = makeEditorView("hello world", { anchor: 0, head: 5 })
        registerView(EDIT_PATH, view)
        useWorkspaceStore.setState({ groups: [{ tabs: [], activePath: EDIT_PATH }], activeGroupIndex: 0 })

        runContextMenuAction("editor", "cmCopy", {})
        await vi.waitFor(() => {
            expect(calls.some((c) => c.cmd === "plugin:clipboard-manager|write_text")).toBe(true)
        })
        expect(
            (calls.find((c) => c.cmd === "plugin:clipboard-manager|write_text")?.args as { text: string }).text
        ).toBe("hello")
        unregisterView(EDIT_PATH)
    })

    it("editor: cmCut 複製並移除選取文字", async () => {
        mockIPC((cmd) =>
            cmd === "log_event" || cmd === "plugin:clipboard-manager|write_text" ? null : undefined
        )
        const view = makeEditorView("hello world", { anchor: 0, head: 5 })
        registerView(EDIT_PATH, view)
        useWorkspaceStore.setState({ groups: [{ tabs: [], activePath: EDIT_PATH }], activeGroupIndex: 0 })

        runContextMenuAction("editor", "cmCut", {})
        await vi.waitFor(() => {
            expect(view.state.doc.toString()).toBe(" world")
        })
        unregisterView(EDIT_PATH)
    })

    it("editor: cmPaste 貼上 clipboard 內容到游標位置", async () => {
        mockIPC((cmd) => {
            if (cmd === "plugin:clipboard-manager|read_text") return "PASTED-"
            return cmd === "log_event" ? null : undefined
        })
        const view = makeEditorView("hello world", { anchor: 0, head: 0 })
        registerView(EDIT_PATH, view)
        useWorkspaceStore.setState({ groups: [{ tabs: [], activePath: EDIT_PATH }], activeGroupIndex: 0 })

        runContextMenuAction("editor", "cmPaste", {})
        await vi.waitFor(() => {
            expect(view.state.doc.toString()).toBe("PASTED-hello world")
        })
        unregisterView(EDIT_PATH)
    })

    it("editor: cmFormatDoc 呼叫 CodeMirror 的 formatDocument command", () => {
        const view = makeEditorView("const x=1")
        registerView(EDIT_PATH, view)
        useWorkspaceStore.setState({ groups: [{ tabs: [], activePath: EDIT_PATH }], activeGroupIndex: 0 })

        runContextMenuAction("editor", "cmFormatDoc", {})
        expect(formatDocument).toHaveBeenCalledWith(view)
        unregisterView(EDIT_PATH)
    })

    it("editor: 沒有 active view 時 cmCut/cmCopy/cmPaste/cmFormatDoc 都是 no-op、不 throw", () => {
        useWorkspaceStore.setState({ groups: [{ tabs: [], activePath: null }], activeGroupIndex: 0 })
        expect(() => runContextMenuAction("editor", "cmCopy", {})).not.toThrow()
        expect(() => runContextMenuAction("editor", "cmCut", {})).not.toThrow()
        expect(() => runContextMenuAction("editor", "cmPaste", {})).not.toThrow()
        expect(() => runContextMenuAction("editor", "cmFormatDoc", {})).not.toThrow()
        expect(formatDocument).not.toHaveBeenCalled()
    })

    it("git: cmFetch 呼叫 gitStore.runOp('fetch', …) → git_fetch_cmd", async () => {
        const calls: string[] = []
        mockIPC((cmd) => {
            calls.push(cmd)
            if (cmd === "git_branches") return { local: [], remote: [] }
            return cmd === "log_event" ? null : null
        })
        runContextMenuAction("git", "cmFetch", {})
        await vi.waitFor(() => expect(calls).toContain("git_fetch_cmd"))
        await vi.waitFor(() => expect(useGitStore.getState().busy).toBeNull())
    })

    it("status: cmPull 呼叫 gitStore.runOp('pull', …) → git_pull_cmd（git／status 共用同一段 dispatch）", async () => {
        const calls: string[] = []
        mockIPC((cmd) => {
            calls.push(cmd)
            if (cmd === "git_branches") return { local: [], remote: [] }
            return cmd === "log_event" ? null : null
        })
        runContextMenuAction("status", "cmPull", {})
        await vi.waitFor(() => expect(calls).toContain("git_pull_cmd"))
        await vi.waitFor(() => expect(useGitStore.getState().busy).toBeNull())
    })

    it("git: cmPush 呼叫 gitStore.runOp('push', …) → git_push_cmd", async () => {
        const calls: string[] = []
        mockIPC((cmd) => {
            calls.push(cmd)
            if (cmd === "git_branches") return { local: [], remote: [] }
            return cmd === "log_event" ? null : null
        })
        runContextMenuAction("git", "cmPush", {})
        await vi.waitFor(() => expect(calls).toContain("git_push_cmd"))
        await vi.waitFor(() => expect(useGitStore.getState().busy).toBeNull())
    })

    it("git/status: cmCopyBranch／cmCopyHash 複製目前分支與 HEAD oid", async () => {
        const calls: Array<{ cmd: string; args: unknown }> = []
        mockIPC((cmd, args) => {
            calls.push({ cmd, args })
            return cmd === "log_event" ? null : undefined
        })
        useGitStore.setState({ status: makeStatus({ branch: "feature/x", headOid: "a".repeat(40) }) })

        runContextMenuAction("git", "cmCopyBranch", {})
        await vi.waitFor(() => {
            expect(calls.some((c) => c.cmd === "plugin:clipboard-manager|write_text")).toBe(true)
        })
        expect(
            (calls.find((c) => c.cmd === "plugin:clipboard-manager|write_text")?.args as { text: string }).text
        ).toBe("feature/x")

        calls.length = 0
        runContextMenuAction("status", "cmCopyHash", {})
        await vi.waitFor(() => {
            expect(calls.some((c) => c.cmd === "plugin:clipboard-manager|write_text")).toBe(true)
        })
        expect(
            (calls.find((c) => c.cmd === "plugin:clipboard-manager|write_text")?.args as { text: string }).text
        ).toBe("a".repeat(40))
    })

    it("agent: cmStop 呼叫 agentStore.cancel() 結束目前 session", () => {
        useAgentStore.setState({
            activeSessionId: "s1",
            sessions: new Map([
                [
                    "s1",
                    {
                        title: "t",
                        agentLabel: "Agent",
                        model: null,
                        tone: "run",
                        transcript: [],
                        availableCommands: [],
                        stopReason: null,
                        stopBadge: null,
                        error: null,
                        queueDepth: null,
                        running: true,
                        pendingTurn: true,
                        metadataTitle: false,
                        cwd: "/w"
                    }
                ]
            ])
        })
        runContextMenuAction("agent", "cmStop", {})
        const session = useAgentStore.getState().sessions.get("s1")
        expect(session?.stopReason).toBe("cancelled")
        expect(session?.running).toBe(false)
    })

    it("sshhost: cmOpenSftp 切到 SFTP tab、聚焦該 host 並開始連線（F5）", () => {
        useSshStore.setState({ hosts: [], sessions: {}, activeHostId: null, pendingAuthHostId: null })
        useSftpStore.getState().reset()
        const host = useSshStore.getState().addHost({
            name: "h",
            host: "h.example.com",
            port: 22,
            user: "u",
            authKind: "password"
        })
        runContextMenuAction("sshhost", "cmOpenSftp", { hostId: host.id })
        expect(useSftpStore.getState().activeTab).toBe("sftp")
        expect(useSshStore.getState().activeHostId).toBe(host.id)
        // Password host → begins the connect flow rather than silently no-op'ing.
        expect(useSshStore.getState().pendingAuthHostId).toBe(host.id)
    })

    it("terminal: cmKill 移除目前 active pane 的 session（terminalStore.removeSession）", () => {
        useWorkspaceStore.setState({ workspacePath: "/w" })
        useTerminalStore.setState({
            sessions: {
                t1: { sessionId: "t1", title: "Terminal 1", workspace: "/w", shell: "", cols: 80, rows: 24 }
            },
            layouts: {
                "/w": { panes: [{ paneId: "t1", sessionId: "t1" }], activePaneId: "t1", splitDirection: null }
            }
        })
        runContextMenuAction("terminal", "cmKill", {})
        expect(useTerminalStore.getState().layouts["/w"].panes).toHaveLength(0)
        expect(useTerminalStore.getState().sessions.t1).toBeUndefined()
    })
})

// PROB-5 後波：檔案操作類 action 接到 fs_* Rust command，並在成功後 refreshTree。
describe("runContextMenuAction — 檔案操作 (PROB-5 後波)", () => {
    function ipcCalls() {
        const calls: Array<{ cmd: string; args: Record<string, unknown> }> = []
        mockIPC((cmd, args) => {
            calls.push({ cmd, args: (args ?? {}) as Record<string, unknown> })
            if (cmd.startsWith("fs_")) return null
            if (cmd === "plugin:dialog|message") return "Ok"
            if (cmd === "plugin:opener|reveal_item_in_dir") return null
            return cmd === "log_event" ? null : undefined
        })
        return calls
    }

    it("explorer: cmNewFile prompt 檔名 → fs_create_file，成功後 openTab + refreshTree", async () => {
        const calls = ipcCalls()
        useWorkspaceStore.setState({
            workspacePath: "/w",
            groups: [{ tabs: [], activePath: null }],
            activeGroupIndex: 0,
            treeRevision: 0
        })
        const promptSpy = vi.spyOn(window, "prompt").mockReturnValue("new.ts")

        runContextMenuAction("explorer", "cmNewFile", {})
        await vi.waitFor(() => expect(calls.some((c) => c.cmd === "fs_create_file")).toBe(true))
        expect(calls.find((c) => c.cmd === "fs_create_file")?.args).toMatchObject({
            workspace: "/w",
            path: "/w/new.ts"
        })
        await vi.waitFor(() =>
            expect(useWorkspaceStore.getState().groups[0].activePath).toBe("/w/new.ts")
        )
        expect(useWorkspaceStore.getState().treeRevision).toBe(1)
        promptSpy.mockRestore()
    })

    it("explorer: cmNewFolder prompt 名稱 → fs_create_dir，成功後 refreshTree（不開 tab）", async () => {
        const calls = ipcCalls()
        useWorkspaceStore.setState({
            workspacePath: "/w",
            groups: [{ tabs: [], activePath: null }],
            activeGroupIndex: 0,
            treeRevision: 0
        })
        const promptSpy = vi.spyOn(window, "prompt").mockReturnValue("assets")

        runContextMenuAction("explorer", "cmNewFolder", {})
        await vi.waitFor(() => expect(calls.some((c) => c.cmd === "fs_create_dir")).toBe(true))
        expect(calls.find((c) => c.cmd === "fs_create_dir")?.args).toMatchObject({
            workspace: "/w",
            path: "/w/assets"
        })
        await vi.waitFor(() => expect(useWorkspaceStore.getState().treeRevision).toBe(1))
        expect(useWorkspaceStore.getState().groups[0].activePath).toBeNull()
        promptSpy.mockRestore()
    })

    it("explorer: cmNewFile 取消 prompt（null）不呼叫後端、不 refreshTree", async () => {
        const calls = ipcCalls()
        useWorkspaceStore.setState({ workspacePath: "/w", treeRevision: 0 })
        const promptSpy = vi.spyOn(window, "prompt").mockReturnValue(null)

        runContextMenuAction("explorer", "cmNewFile", {})
        await new Promise((resolve) => setTimeout(resolve, 0))
        expect(calls.some((c) => c.cmd === "fs_create_file")).toBe(false)
        expect(useWorkspaceStore.getState().treeRevision).toBe(0)
        promptSpy.mockRestore()
    })

    it("file: cmRename prompt 新名稱 → fs_rename（同目錄換 basename），成功後 refreshTree", async () => {
        const calls = ipcCalls()
        useWorkspaceStore.setState({ workspacePath: "/w", treeRevision: 0 })
        const promptSpy = vi.spyOn(window, "prompt").mockReturnValue("renamed.ts")

        runContextMenuAction("file", "cmRename", { path: "/w/src/old.ts" })
        await vi.waitFor(() => expect(calls.some((c) => c.cmd === "fs_rename")).toBe(true))
        expect(calls.find((c) => c.cmd === "fs_rename")?.args).toMatchObject({
            workspace: "/w",
            from: "/w/src/old.ts",
            to: "/w/src/renamed.ts"
        })
        await vi.waitFor(() => expect(useWorkspaceStore.getState().treeRevision).toBe(1))
        promptSpy.mockRestore()
    })

    it("file: cmRename 名稱未變（等於原名）不呼叫後端", async () => {
        const calls = ipcCalls()
        useWorkspaceStore.setState({ workspacePath: "/w", treeRevision: 0 })
        const promptSpy = vi.spyOn(window, "prompt").mockReturnValue("old.ts")

        runContextMenuAction("file", "cmRename", { path: "/w/src/old.ts" })
        await new Promise((resolve) => setTimeout(resolve, 0))
        expect(calls.some((c) => c.cmd === "fs_rename")).toBe(false)
        promptSpy.mockRestore()
    })

    it("file: cmDelete 檔案 → confirm（文字含檔名、不含資料夾警告）→ fs_delete + refreshTree", async () => {
        const calls = ipcCalls()
        useWorkspaceStore.setState({ workspacePath: "/w", treeRevision: 0 })

        runContextMenuAction("file", "cmDelete", { path: "/w/f.ts", isDir: false })
        await vi.waitFor(() => expect(calls.some((c) => c.cmd === "fs_delete")).toBe(true))
        const confirmCall = calls.find((c) => c.cmd === "plugin:dialog|message")
        expect(String(confirmCall?.args.message)).toContain("f.ts")
        expect(String(confirmCall?.args.message)).not.toContain("資料夾")
        expect(calls.find((c) => c.cmd === "fs_delete")?.args).toMatchObject({
            workspace: "/w",
            path: "/w/f.ts"
        })
        await vi.waitFor(() => expect(useWorkspaceStore.getState().treeRevision).toBe(1))
    })

    it("file: cmDelete 資料夾 → confirm 文字帶「將刪除整個資料夾」警告", async () => {
        const calls = ipcCalls()
        useWorkspaceStore.setState({ workspacePath: "/w", treeRevision: 0 })

        runContextMenuAction("file", "cmDelete", { path: "/w/dir", isDir: true })
        await vi.waitFor(() => expect(calls.some((c) => c.cmd === "fs_delete")).toBe(true))
        const confirmCall = calls.find((c) => c.cmd === "plugin:dialog|message")
        expect(String(confirmCall?.args.message)).toContain("將刪除整個資料夾")
    })

    it("file: cmDelete 取消 confirm 不呼叫 fs_delete、不 refreshTree", async () => {
        const calls: Array<{ cmd: string }> = []
        mockIPC((cmd) => {
            calls.push({ cmd })
            if (cmd === "plugin:dialog|message") return "Cancel"
            return cmd === "log_event" ? null : undefined
        })
        useWorkspaceStore.setState({ workspacePath: "/w", treeRevision: 0 })

        runContextMenuAction("file", "cmDelete", { path: "/w/f.ts", isDir: false })
        await new Promise((resolve) => setTimeout(resolve, 0))
        expect(calls.some((c) => c.cmd === "fs_delete")).toBe(false)
        expect(useWorkspaceStore.getState().treeRevision).toBe(0)
    })

    it("file: cmReveal 呼叫 opener 的 reveal_item_in_dir（傳目標 path）", async () => {
        const calls = ipcCalls()
        runContextMenuAction("file", "cmReveal", { path: "/w/f.ts" })
        await vi.waitFor(() =>
            expect(calls.some((c) => c.cmd === "plugin:opener|reveal_item_in_dir")).toBe(true)
        )
        expect(calls.find((c) => c.cmd === "plugin:opener|reveal_item_in_dir")?.args).toMatchObject({
            paths: ["/w/f.ts"]
        })
    })

    // P3: serve the html file's own dir, then open the served URL in the
    // singleton preview tab (same dir reuses one port on the Rust side).
    it("file: cmOpenInBrowser preview_serve 該檔目錄並在 preview 分頁開啟服務網址", async () => {
        const calls: Array<{ cmd: string; args: Record<string, unknown> }> = []
        mockIPC((cmd, args) => {
            calls.push({ cmd, args: (args ?? {}) as Record<string, unknown> })
            if (cmd === "preview_serve") return 4599
            return cmd === "log_event" ? null : undefined
        })
        useWorkspaceStore.setState({
            workspacePath: "/w",
            groups: [{ tabs: [], activePath: null }],
            activeGroupIndex: 0
        })

        runContextMenuAction("file", "cmOpenInBrowser", { path: "/w/site/index.html" })

        await vi.waitFor(() =>
            expect(usePreviewStore.getState().navForWorkspace("/w").url).toBe(
                "http://127.0.0.1:4599/index.html"
            )
        )
        expect(calls.find((c) => c.cmd === "preview_serve")?.args).toMatchObject({ dir: "/w/site" })
        const g = useWorkspaceStore.getState().groups[0]
        expect(g.tabs.some((t) => t.kind === "preview")).toBe(true)
        expect(g.activePath).toBe("yuzora://preview")
    })

    // Finding 3 (Codex high): a delete/rename that leaves the open tab pointing at
    // the old path lets its EditorPane recreate/mis-target the file on the next
    // save. cmDelete must close those tabs; cmRename must re-point them.
    it("file: cmDelete 檔案 → 關閉所有指向該 path 的 tab（跨所有 group），並修正 activePath", async () => {
        ipcCalls()
        useWorkspaceStore.setState({
            workspacePath: "/w",
            treeRevision: 0,
            activeGroupIndex: 0,
            groups: [
                {
                    activePath: "/w/f.ts",
                    tabs: [
                        { path: "/w/f.ts", name: "f.ts", dirty: false, externallyModified: false },
                        { path: "/w/keep.ts", name: "keep.ts", dirty: false, externallyModified: false }
                    ]
                },
                {
                    activePath: "/w/f.ts",
                    tabs: [{ path: "/w/f.ts", name: "f.ts", dirty: false, externallyModified: false }]
                }
            ]
        })
        runContextMenuAction("file", "cmDelete", { path: "/w/f.ts", isDir: false })
        await vi.waitFor(() => {
            const gs = useWorkspaceStore.getState().groups
            expect(gs[0].tabs.some((t) => t.path === "/w/f.ts")).toBe(false)
            expect(gs[1].tabs.some((t) => t.path === "/w/f.ts")).toBe(false)
        })
        const gs = useWorkspaceStore.getState().groups
        expect(gs[0].tabs.map((t) => t.path)).toEqual(["/w/keep.ts"])
        expect(gs[0].activePath).toBe("/w/keep.ts")
        expect(gs[1].tabs).toEqual([])
        expect(gs[1].activePath).toBeNull()
    })

    it("file: cmDelete 資料夾 → 關閉該資料夾底下的所有 tab、保留資料夾外的", async () => {
        ipcCalls()
        useWorkspaceStore.setState({
            workspacePath: "/w",
            treeRevision: 0,
            activeGroupIndex: 0,
            groups: [
                {
                    activePath: "/w/dir/a.ts",
                    tabs: [
                        { path: "/w/dir/a.ts", name: "a.ts", dirty: false, externallyModified: false },
                        { path: "/w/dir/sub/b.ts", name: "b.ts", dirty: false, externallyModified: false },
                        { path: "/w/outside.ts", name: "outside.ts", dirty: false, externallyModified: false }
                    ]
                }
            ]
        })
        runContextMenuAction("file", "cmDelete", { path: "/w/dir", isDir: true })
        await vi.waitFor(() => {
            expect(useWorkspaceStore.getState().groups[0].tabs.map((t) => t.path)).toEqual([
                "/w/outside.ts"
            ])
        })
        expect(useWorkspaceStore.getState().groups[0].activePath).toBe("/w/outside.ts")
    })

    it("file: cmRename → 受影響 tab 的 path/name 改為新路徑（dirty 保留、不留 stale 舊 path）", async () => {
        ipcCalls()
        useWorkspaceStore.setState({
            workspacePath: "/w",
            treeRevision: 0,
            activeGroupIndex: 0,
            groups: [
                {
                    activePath: "/w/src/old.ts",
                    tabs: [
                        { path: "/w/src/old.ts", name: "old.ts", dirty: true, externallyModified: false }
                    ]
                }
            ]
        })
        const promptSpy = vi.spyOn(window, "prompt").mockReturnValue("renamed.ts")
        runContextMenuAction("file", "cmRename", { path: "/w/src/old.ts" })
        await vi.waitFor(() => {
            expect(
                useWorkspaceStore.getState().groups[0].tabs.some((t) => t.path === "/w/src/renamed.ts")
            ).toBe(true)
        })
        const g = useWorkspaceStore.getState().groups[0]
        expect(g.tabs.some((t) => t.path === "/w/src/old.ts")).toBe(false)
        expect(g.tabs[0].name).toBe("renamed.ts")
        expect(g.tabs[0].dirty).toBe(true)
        expect(g.activePath).toBe("/w/src/renamed.ts")
        promptSpy.mockRestore()
    })
})

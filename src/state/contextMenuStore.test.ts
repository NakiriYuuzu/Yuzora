import { beforeEach, describe, expect, it, vi } from "vitest"
import { clearMocks, mockIPC } from "@tauri-apps/api/mocks"
import { EditorState } from "@codemirror/state"
import { EditorView } from "@codemirror/view"
import type { MouseEvent as ReactMouseEvent } from "react"

import {
    contextMenuHandler,
    executeLegacyContextMenuAction,
    runContextMenuAction,
    useContextMenuStore
} from "@/state/contextMenuStore"
import { commandFor } from "@/app/workbench/contextMenuDefs"
import type { ContextMenuKind, ContextMenuRequest } from "@/app/workbench/contextMenuModel"
import { useAgentStore, type SessionState } from "@/state/agentStore"
import { useDbStore } from "@/state/dbStore"
import { useDiffModalStore } from "@/state/diffModalStore"
import { useGitStore } from "@/state/gitStore"
import { usePreviewStore } from "@/state/previewStore"
import { useSftpStore } from "@/state/sftpStore"
import { useSshStore } from "@/state/sshStore"
import { useSvgPreviewStore } from "@/state/svgPreviewStore"
import { useTerminalStore, terminalInitialState } from "@/state/terminalStore"
import { useUiStore, uiInitialState } from "@/state/uiStore"
import { useWorkspaceStore } from "@/state/workspaceStore"
import { registerView, unregisterView } from "@/editor/viewRegistry"
import type { GitStatus } from "@/lib/types"

// Registered under the view registry so runLegacyContextMenuAction("editor", …) can
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

function makeAgentSession(overrides: Partial<SessionState> = {}): SessionState {
    return {
        title: "Session",
        agentLabel: "Agent",
        model: null,
        tone: "idle",
        transcript: [],
        availableCommands: [],
        stopReason: null,
        stopBadge: null,
        error: null,
        queueDepth: null,
        running: null,
        pendingTurn: false,
        metadataTitle: false,
        cwd: "/w",
        ...overrides
    }
}

const originalAgentCancel = useAgentStore.getState().cancel

type LegacyKind = Exclude<ContextMenuKind, "agentSession" | "gitChange" | "preview">
interface LegacyPayload {
    path?: string
    groupIndex?: number
    isDir?: boolean
    hostId?: string
    host?: string
    descriptorId?: string
    addr?: string
    sessionId?: string
}

function legacyRequest(kind: LegacyKind, payload: LegacyPayload = {}): ContextMenuRequest {
    const workspace = useWorkspaceStore.getState()
    const git = useGitStore.getState()
    if (kind === "general" || kind === "rail") return { kind }
    if (kind === "explorer") return { kind, workspacePath: workspace.workspacePath }
    if (kind === "file") {
        return {
            kind,
            workspacePath: workspace.workspacePath ?? "/w",
            path: payload.path ?? "/w/file.ts",
            isDirectory: payload.isDir ?? false,
            sourceGroupIndex: payload.groupIndex ?? workspace.activeGroupIndex,
        }
    }
    if (kind === "tab") {
        return {
            kind,
            workspacePath: workspace.workspacePath,
            path: payload.path ?? "/w/file.ts",
            groupIndex: payload.groupIndex ?? workspace.activeGroupIndex,
        }
    }
    if (kind === "editor") {
        return {
            kind,
            workspacePath: workspace.workspacePath,
            path: payload.path ?? workspace.groups[workspace.activeGroupIndex]?.activePath ?? EDIT_PATH,
            groupIndex: payload.groupIndex ?? workspace.activeGroupIndex,
        }
    }
    if (kind === "git" || kind === "status") {
        return {
            kind,
            repositoryRoot: git.environment?.status === "ready" ? git.environment.root : null,
        }
    }
    if (kind === "sshhost") {
        return { kind, hostId: payload.hostId ?? "missing-host", address: payload.host ?? "" }
    }
    return {
        kind: "dbconn",
        descriptorId: payload.descriptorId ?? "missing-descriptor",
        address: payload.addr ?? "",
    }
}

function runLegacyContextMenuAction(kind: LegacyKind, actionId: string, payload: LegacyPayload = {}) {
    return executeLegacyContextMenuAction(legacyRequest(kind, payload), actionId)
}

beforeEach(() => {
    clearMocks()
    mockIPC((cmd) => (cmd === "log_event" ? null : undefined))
    vi.clearAllMocks()
    useContextMenuStore.setState({ request: null, x: 0, y: 0, availabilityRevision: 0 })
    useDiffModalStore.setState({ open: false, source: null, activeIndex: 0, mode: "unified" })
    useSvgPreviewStore.getState().reset()
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
        authRequired: null,
        pendingNewSession: false,
        renamingSessionId: null,
        confirmRemoveRequest: null,
        cancel: originalAgentCancel
    })
})

describe("contextMenuStore", () => {
    it("open 原子保存 typed request 與座標；close 清掉 request", () => {
        const request: ContextMenuRequest = {
            kind: "tab",
            workspacePath: "/w",
            path: "/a.md",
            groupIndex: 0,
        }
        useContextMenuStore.getState().open(request, 120, 240)
        expect(useContextMenuStore.getState()).toMatchObject({
            request,
            x: 120,
            y: 240,
        })
        useContextMenuStore.getState().close()
        expect(useContextMenuStore.getState().request).toBeNull()
    })

    it("contextMenuHandler 阻止原生選單、停止冒泡並開啟對應選單", () => {
        const event = {
            preventDefault: vi.fn(),
            stopPropagation: vi.fn(),
            clientX: 33,
            clientY: 44
        } as unknown as ReactMouseEvent
        contextMenuHandler({ kind: "rail" })(event)
        expect(event.preventDefault).toHaveBeenCalled()
        expect(event.stopPropagation).toHaveBeenCalled()
        expect(useContextMenuStore.getState()).toMatchObject({ request: { kind: "rail" }, x: 33, y: 44 })
    })

    it("completed 才關閉後記 user-action log", async () => {
        const logged: unknown[] = []
        mockIPC((cmd, args) => {
            if (cmd === "log_event") {
                logged.push(args)
                return null
            }
            return undefined
        })
        const request: ContextMenuRequest = { kind: "general" }
        const command = commandFor(request, "cmSettings")
        expect(command).not.toBeNull()
        useContextMenuStore.getState().open(request, 0, 0)
        await runContextMenuAction(request, command!)
        expect(useContextMenuStore.getState().request).toBeNull()
        expect(logged).toHaveLength(1)
        const entry = (logged[0] as { event: { event: string; message: string; metadata: { kind: string } } }).event
        expect(entry.event).toBe("context_menu_action")
        expect(entry.message).toBe("general:cmSettings")
        expect(entry.metadata.kind).toBe("general")
    })

    it("stale preflight disabled 時保留 menu、重算 reason，且不 dispatch/log", async () => {
        const logged: unknown[] = []
        mockIPC((cmd, args) => {
            if (cmd === "log_event") logged.push(args)
            return null
        })
        const request: ContextMenuRequest = { kind: "general" }
        const executor = vi.fn(async () => "completed" as const)
        const command = {
            id: "stale",
            label: () => "Stale",
            availability: () => ({
                visible: true,
                enabled: false,
                disabledReasonKey: "contextMenu.disabled.targetUnavailable",
            }),
            danger: false,
            executor,
        }
        useContextMenuStore.getState().open(request, 1, 2)
        const before = useContextMenuStore.getState().availabilityRevision
        expect(await runContextMenuAction(request, command)).toBe("cancelled")
        expect(useContextMenuStore.getState().request).toBe(request)
        expect(useContextMenuStore.getState().availabilityRevision).toBe(before + 1)
        expect(executor).not.toHaveBeenCalled()
        expect(logged).toHaveLength(0)
    })

    it("cancelled outcome 關閉 menu 但不記成功 log", async () => {
        const logged: unknown[] = []
        mockIPC((cmd, args) => {
            if (cmd === "log_event") logged.push(args)
            return null
        })
        const request: ContextMenuRequest = { kind: "general" }
        const command = {
            id: "cancelled",
            label: () => "Cancelled",
            availability: () => ({ visible: true, enabled: true }),
            danger: false,
            executor: async () => "cancelled" as const,
        }
        useContextMenuStore.getState().open(request, 1, 2)
        expect(await runContextMenuAction(request, command)).toBe("cancelled")
        expect(useContextMenuStore.getState().request).toBeNull()
        expect(logged).toHaveLength(0)
    })

    it("executor throw 交 actionFeedback，回傳 error 且不記成功 log", async () => {
        const calls: Array<{ cmd: string; args: unknown }> = []
        mockIPC((cmd, args) => {
            calls.push({ cmd, args })
            return cmd === "plugin:dialog|message" ? "Ok" : null
        })
        const request: ContextMenuRequest = { kind: "general" }
        const command = {
            id: "throws",
            label: () => "Throwing action",
            availability: () => ({ visible: true, enabled: true }),
            danger: false,
            executor: async (): Promise<"completed"> => {
                throw new Error("safe failure")
            },
        }
        useContextMenuStore.getState().open(request, 1, 2)
        expect(await runContextMenuAction(request, command)).toBe("error")
        expect(calls.some((call) => call.cmd === "plugin:dialog|message")).toBe(true)
        expect(calls.some((call) => call.cmd === "log_event")).toBe(false)
    })

    it("cmCompareHead 把 clicked path 相對化後對『在變更清單中的檔案』開 Diff modal", async () => {
        // Editor tabs open with ABSOLUTE paths (FileTree passes node.path); git
        // status paths are repo-relative. cmCompareHead must strip the repo root
        // before matching — otherwise this entry is a永久 no-op in production.
        useWorkspaceStore.setState({
            groups: [
                { tabs: [], activePath: "/w/src/git.rs" },
                { tabs: [], activePath: "/w/src/other.rs" }
            ],
            activeGroupIndex: 1
        })
        const view = makeEditorView("changed")
        registerView("/w/src/git.rs", view, { groupIndex: 0 })
        useGitStore.setState({
            environment: { status: "ready", root: "/w", version: "2.50" },
            status: makeStatus({ unstaged: [{ path: "src/git.rs", origPath: null, status: "M" }] })
        })
        await runLegacyContextMenuAction("editor", "cmCompareHead", {
            path: "/w/src/git.rs",
            groupIndex: 0
        })
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
        unregisterView("/w/src/git.rs", view)
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
        runLegacyContextMenuAction("editor", "cmCompareHead", {})
        expect(useDiffModalStore.getState().open).toBe(false)
        expect(useUiStore.getState().mode).toBe(uiInitialState.mode)
    })

    it("cmCompareHead 無 active 檔時 no-op", () => {
        useGitStore.setState({
            environment: { status: "ready", root: "/w", version: "2.50" },
            status: makeStatus({ unstaged: [{ path: "src/git.rs", origPath: null, status: "M" }] })
        })
        runLegacyContextMenuAction("editor", "cmCompareHead", {})
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
        runLegacyContextMenuAction("editor", "cmCompareHead", {})
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
        runLegacyContextMenuAction("editor", "cmCompareHead", {})
        expect(useDiffModalStore.getState().open).toBe(false)
    })
})

describe("agentSession command preflight", () => {
    it("非 pending 的 clicked session 保持 menu 開啟、顯示原因且不 fallback 到 pending active session", async () => {
        const cancel = vi.fn(async () => true)
        const request: ContextMenuRequest = { kind: "agentSession", sessionId: "clicked" }
        useAgentStore.setState({
            activeSessionId: "active",
            cancel,
            sessions: new Map([
                ["clicked", makeAgentSession({ pendingTurn: false })],
                ["active", makeAgentSession({ pendingTurn: true })]
            ])
        })
        const command = commandFor(request, "cmCancelResponse")
        if (!command) throw new Error("missing cmCancelResponse")
        expect(command.availability(request)).toEqual({
            visible: true,
            enabled: false,
            disabledReasonKey: "contextMenu.disabled.noPendingResponse"
        })
        useContextMenuStore.getState().open(request, 10, 20)

        expect(await runContextMenuAction(request, command)).toBe("cancelled")
        expect(useContextMenuStore.getState().request).toBe(request)
        expect(cancel).not.toHaveBeenCalled()
    })

    it("pending clicked session 只 dispatch clicked sessionId，不使用 activeSessionId", async () => {
        const cancel = vi.fn(async () => true)
        const request: ContextMenuRequest = { kind: "agentSession", sessionId: "clicked" }
        useAgentStore.setState({
            activeSessionId: "active",
            cancel,
            sessions: new Map([
                ["clicked", makeAgentSession({ pendingTurn: true })],
                ["active", makeAgentSession({ pendingTurn: true })]
            ])
        })
        const command = commandFor(request, "cmCancelResponse")
        if (!command) throw new Error("missing cmCancelResponse")
        useContextMenuStore.getState().open(request, 10, 20)

        expect(await runContextMenuAction(request, command)).toBe("completed")
        expect(cancel).toHaveBeenCalledTimes(1)
        expect(cancel).toHaveBeenCalledWith("clicked")
        expect(useAgentStore.getState().activeSessionId).toBe("active")
    })

    it("Cancel Response failure reaches actionFeedback and preserves the clicked pending turn", async () => {
        const calls: string[] = []
        mockIPC((cmd) => {
            calls.push(cmd)
            return cmd === "plugin:dialog|message" ? "Ok" : null
        })
        const cancel = vi.fn(async () => {
            throw new Error("cancel failed")
        })
        const request: ContextMenuRequest = { kind: "agentSession", sessionId: "clicked" }
        useAgentStore.setState({
            activeSessionId: "other",
            connection: { cancel } as never,
            sessions: new Map([
                ["clicked", makeAgentSession({ pendingTurn: true, running: true, tone: "run" })],
                ["other", makeAgentSession({ pendingTurn: true })]
            ])
        })
        const command = commandFor(request, "cmCancelResponse")
        if (!command) throw new Error("missing cmCancelResponse")
        useContextMenuStore.getState().open(request, 10, 20)

        expect(await runContextMenuAction(request, command)).toBe("error")
        expect(cancel).toHaveBeenCalledWith("clicked")
        expect(useAgentStore.getState().sessions.get("clicked")).toMatchObject({
            pendingTurn: true,
            running: true,
            tone: "run"
        })
        expect(useAgentStore.getState().sessions.get("other")?.pendingTurn).toBe(true)
        expect(calls).toContain("plugin:dialog|message")
        expect(calls).not.toContain("log_event")
    })

    it("Remove Session confirms first, then keeps a pending session when cancel fails", async () => {
        const calls: string[] = []
        mockIPC((cmd) => {
            calls.push(cmd)
            return cmd === "plugin:dialog|message" ? "Ok" : null
        })
        const cancel = vi.fn(async () => {
            throw new Error("cancel refused")
        })
        const dropSession = vi.fn()
        const request: ContextMenuRequest = { kind: "agentSession", sessionId: "clicked" }
        useAgentStore.setState({
            connection: { cancel, dropSession } as never,
            sessions: new Map([["clicked", makeAgentSession({ pendingTurn: true, running: true })]])
        })
        const command = commandFor(request, "cmRemoveSession")
        if (!command) throw new Error("missing cmRemoveSession")
        useContextMenuStore.getState().open(request, 10, 20)

        const removing = runContextMenuAction(request, command)
        useAgentStore.getState().respondRemoveSessionConfirm(true)

        expect(await removing).toBe("error")
        expect(useAgentStore.getState().sessions.has("clicked")).toBe(true)
        expect(dropSession).not.toHaveBeenCalled()
        expect(calls).toContain("plugin:dialog|message")
        expect(calls).not.toContain("log_event")
    })

    it("Copy Working Directory clipboard failure 交給 actionFeedback 顯示，不冒充成功", async () => {
        const calls: string[] = []
        mockIPC((cmd) => {
            calls.push(cmd)
            if (cmd === "plugin:clipboard-manager|write_text") throw new Error("clipboard denied")
            return cmd === "plugin:dialog|message" ? "Ok" : null
        })
        const request: ContextMenuRequest = { kind: "agentSession", sessionId: "clicked" }
        useAgentStore.setState({
            sessions: new Map([["clicked", makeAgentSession({ cwd: "/clicked/project" })]])
        })
        const command = commandFor(request, "cmCopyWorkingDirectory")
        if (!command) throw new Error("missing cmCopyWorkingDirectory")
        useContextMenuStore.getState().open(request, 10, 20)

        expect(await runContextMenuAction(request, command)).toBe("error")
        expect(calls).toContain("plugin:clipboard-manager|write_text")
        expect(calls).toContain("plugin:dialog|message")
        expect(calls).not.toContain("log_event")
    })
})

// PROB-5 前波：純前端可完成的 action 真的接到目標 API（而非只 close+log）。
describe("runContextMenuAction — 前端接線 (PROB-5)", () => {
    it("cmSettings 開啟 Settings dialog（general／rail 共用）", () => {
        runLegacyContextMenuAction("general", "cmSettings", {})
        expect(useUiStore.getState().settingsOpen).toBe(true)
    })

    it("cmHideSidebar bump sidebarToggleRequest（AppShell 據此切換 navCollapsed，見該 store）", () => {
        const before = useUiStore.getState().sidebarToggleRequest
        runLegacyContextMenuAction("rail", "cmHideSidebar", {})
        expect(useUiStore.getState().sidebarToggleRequest).toBe(before + 1)
    })

    it("cmCmdPalette（general／editor 共用）bump paletteOpenRequest", () => {
        const before = useUiStore.getState().paletteOpenRequest
        runLegacyContextMenuAction("general", "cmCmdPalette", {})
        expect(useUiStore.getState().paletteOpenRequest).toBe(before + 1)
        runLegacyContextMenuAction("editor", "cmCmdPalette", {})
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
        runLegacyContextMenuAction("tab", "cmCloseTab", { path: "/w/b.ts", groupIndex: 0 })
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
        runLegacyContextMenuAction("tab", "cmCloseTab", { path: "/w/b.ts", groupIndex: 0 })
        await vi.waitFor(() => {
            expect(useWorkspaceStore.getState().groups[0].tabs.some((t) => t.path === "/w/b.ts")).toBe(true)
        })

        mockIPC((cmd) => {
            if (cmd === "plugin:dialog|message") return "Ok"
            return cmd === "log_event" ? null : undefined
        })
        runLegacyContextMenuAction("tab", "cmCloseTab", { path: "/w/b.ts", groupIndex: 0 })
        await vi.waitFor(() => {
            expect(useWorkspaceStore.getState().groups[0].tabs.some((t) => t.path === "/w/b.ts")).toBe(false)
        })
    })

    it("tab: dirty confirm 顯示 Windows basename，但 request identity 保留 raw path", async () => {
        const rawPath = String.raw`\\?\C:\Work\中文 workspace\a.ts`
        const calls: Array<{ cmd: string; args: Record<string, unknown> }> = []
        mockIPC((cmd, args) => {
            calls.push({ cmd, args: (args ?? {}) as Record<string, unknown> })
            if (cmd === "plugin:dialog|message") return "Cancel"
            return cmd === "log_event" ? null : undefined
        })
        useWorkspaceStore.setState({
            workspacePath: String.raw`\\?\C:\Work\中文 workspace`,
            activeGroupIndex: 0,
            groups: [{
                activePath: rawPath,
                tabs: [{ path: rawPath, name: rawPath, dirty: true, externallyModified: false }]
            }]
        })

        runLegacyContextMenuAction("tab", "cmCloseTab", { path: rawPath, groupIndex: 0 })

        await vi.waitFor(() => expect(calls.some((call) => call.cmd === "plugin:dialog|message")).toBe(true))
        const message = String(calls.find((call) => call.cmd === "plugin:dialog|message")?.args.message)
        expect(message).toContain("a.ts")
        expect(message).not.toContain(rawPath)
        expect(useWorkspaceStore.getState().groups[0].tabs[0].path).toBe(rawPath)
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
        runLegacyContextMenuAction("tab", "cmCloseOthers", { path: "/w/b.ts", groupIndex: 0 })
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
        runLegacyContextMenuAction("tab", "cmCloseAll", { groupIndex: 0 })
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

        runLegacyContextMenuAction("tab", "cmCopyRel", { path: "/w/src/a.ts", groupIndex: 0 })
        await vi.waitFor(() => {
            expect(calls.some((c) => c.cmd === "plugin:clipboard-manager|write_text")).toBe(true)
        })
        expect(
            (calls.find((c) => c.cmd === "plugin:clipboard-manager|write_text")?.args as { text: string }).text
        ).toBe("src/a.ts")

        calls.length = 0
        runLegacyContextMenuAction("file", "cmCopyRel", { path: "/other/a.ts" })
        await vi.waitFor(() => {
            expect(calls.some((c) => c.cmd === "plugin:clipboard-manager|write_text")).toBe(true)
        })
        expect(
            (calls.find((c) => c.cmd === "plugin:clipboard-manager|write_text")?.args as { text: string }).text
        ).toBe("/other/a.ts")
    })

    it("tab/file: split action 執行 atomic move/open，不留下空 group", async () => {
        useWorkspaceStore.setState({ workspacePath: "/w" })
        const tab = { path: "/w/a.ts", name: "a.ts", dirty: true, externallyModified: true }
        useWorkspaceStore.setState({ groups: [{ tabs: [tab], activePath: tab.path }], activeGroupIndex: 0 })
        expect(await runLegacyContextMenuAction("tab", "cmSplit", { path: tab.path, groupIndex: 0 })).toBe("completed")
        expect(useWorkspaceStore.getState().groups).toHaveLength(2)
        expect(useWorkspaceStore.getState().groups[0].tabs).toEqual([])
        expect(useWorkspaceStore.getState().groups[1].tabs[0]).toMatchObject({
            path: tab.path,
            dirty: true,
            externallyModified: true
        })

        useWorkspaceStore.setState({ groups: [{ tabs: [], activePath: null }], activeGroupIndex: 0 })
        expect(await runLegacyContextMenuAction("file", "cmOpenSplit", { path: "/w/b.ts" })).toBe("completed")
        expect(useWorkspaceStore.getState().groups).toHaveLength(2)
        expect(useWorkspaceStore.getState().groups[1].activePath).toBe("/w/b.ts")
    })

    it("file: cmOpen 開啟該檔案分頁", () => {
        useWorkspaceStore.setState({ groups: [{ tabs: [], activePath: null }], activeGroupIndex: 0 })
        runLegacyContextMenuAction("file", "cmOpen", { path: "/w/a.ts" })
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
        runLegacyContextMenuAction("explorer", "cmCopyPath", {})
        await vi.waitFor(() => {
            expect(calls.some((c) => c.cmd === "plugin:clipboard-manager|write_text")).toBe(true)
        })
        expect(
            (calls.find((c) => c.cmd === "plugin:clipboard-manager|write_text")?.args as { text: string }).text
        ).toBe("/w/project")
    })

    it("editor: cmCopy 複製選取文字到 clipboard", async () => {
        const calls: Array<{ cmd: string; args: unknown }> = []
        mockIPC((cmd, args) => {
            calls.push({ cmd, args })
            return cmd === "log_event" ? null : undefined
        })
        const view = makeEditorView("hello world", { anchor: 0, head: 5 })
        registerView(EDIT_PATH, view, { groupIndex: 0 })
        useWorkspaceStore.setState({ groups: [{ tabs: [], activePath: EDIT_PATH }], activeGroupIndex: 0 })

        runLegacyContextMenuAction("editor", "cmCopy", {})
        await vi.waitFor(() => {
            expect(calls.some((c) => c.cmd === "plugin:clipboard-manager|write_text")).toBe(true)
        })
        expect(
            (calls.find((c) => c.cmd === "plugin:clipboard-manager|write_text")?.args as { text: string }).text
        ).toBe("hello")
        unregisterView(EDIT_PATH)
    })

    it("editor actions use the clicked path/group even when another group is active", async () => {
        const calls: Array<{ cmd: string; args: unknown }> = []
        mockIPC((cmd, args) => {
            calls.push({ cmd, args })
            return cmd === "log_event" ? null : undefined
        })
        const clickedPath = "/w/clicked.ts"
        const activePath = "/w/active.ts"
        const clicked = makeEditorView("clicked", { anchor: 0, head: 7 })
        const active = makeEditorView("active", { anchor: 0, head: 6 })
        registerView(clickedPath, clicked, { groupIndex: 0 })
        registerView(activePath, active, { groupIndex: 1 })
        useWorkspaceStore.setState({
            workspacePath: "/w",
            activeGroupIndex: 1,
            groups: [
                { tabs: [], activePath: clickedPath },
                { tabs: [], activePath }
            ]
        })

        await runLegacyContextMenuAction("editor", "cmCopy", {
            path: clickedPath,
            groupIndex: 0
        })

        const write = calls.find((call) => call.cmd === "plugin:clipboard-manager|write_text")
        expect((write?.args as { text: string }).text).toBe("clicked")
        unregisterView(clickedPath, clicked)
        unregisterView(activePath, active)
    })

    it("editor: cmCut 複製並移除選取文字", async () => {
        mockIPC((cmd) =>
            cmd === "log_event" || cmd === "plugin:clipboard-manager|write_text" ? null : undefined
        )
        const view = makeEditorView("hello world", { anchor: 0, head: 5 })
        registerView(EDIT_PATH, view, { groupIndex: 0 })
        useWorkspaceStore.setState({ groups: [{ tabs: [], activePath: EDIT_PATH }], activeGroupIndex: 0 })

        runLegacyContextMenuAction("editor", "cmCut", {})
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
        registerView(EDIT_PATH, view, { groupIndex: 0 })
        useWorkspaceStore.setState({ groups: [{ tabs: [], activePath: EDIT_PATH }], activeGroupIndex: 0 })

        runLegacyContextMenuAction("editor", "cmPaste", {})
        await vi.waitFor(() => {
            expect(view.state.doc.toString()).toBe("PASTED-hello world")
        })
        unregisterView(EDIT_PATH)
    })

    it("editor: cmFormatDoc 只呼叫 clicked view 註冊的 formatter", async () => {
        const view = makeEditorView("const x=1")
        const format = vi.fn(async () => true)
        registerView(EDIT_PATH, view, {
            groupIndex: 0,
            formatter: "available",
            formatDocument: format
        })
        useWorkspaceStore.setState({ groups: [{ tabs: [], activePath: EDIT_PATH }], activeGroupIndex: 0 })

        expect(await runLegacyContextMenuAction("editor", "cmFormatDoc", {})).toBe("completed")
        expect(format).toHaveBeenCalledTimes(1)
        unregisterView(EDIT_PATH)
    })

    it("editor: Paste clipboard failure reaches the shared action-error dialog", async () => {
        const calls: string[] = []
        mockIPC((cmd) => {
            calls.push(cmd)
            if (cmd === "plugin:clipboard-manager|read_text") throw new Error("clipboard denied")
            return null
        })
        const view = makeEditorView("hello")
        registerView(EDIT_PATH, view, { groupIndex: 0 })
        useWorkspaceStore.setState({
            workspacePath: "/w",
            groups: [{ tabs: [], activePath: EDIT_PATH }],
            activeGroupIndex: 0
        })
        const request: ContextMenuRequest = {
            kind: "editor",
            workspacePath: "/w",
            path: EDIT_PATH,
            groupIndex: 0
        }
        const command = commandFor(request, "cmPaste")!
        useContextMenuStore.getState().open(request, 1, 1)

        expect(await runContextMenuAction(request, command)).toBe("error")
        expect(calls).toContain("plugin:dialog|message")
        unregisterView(EDIT_PATH, view)
    })

    it("editor: formatter rejection reaches the shared action-error dialog", async () => {
        const calls: string[] = []
        mockIPC((cmd) => {
            calls.push(cmd)
            return null
        })
        const view = makeEditorView("hello")
        registerView(EDIT_PATH, view, {
            groupIndex: 0,
            formatter: "available",
            formatDocument: async () => {
                throw new Error("formatter failed")
            }
        })
        useWorkspaceStore.setState({
            workspacePath: "/w",
            groups: [{ tabs: [], activePath: EDIT_PATH }],
            activeGroupIndex: 0
        })
        const request: ContextMenuRequest = {
            kind: "editor",
            workspacePath: "/w",
            path: EDIT_PATH,
            groupIndex: 0
        }
        const command = commandFor(request, "cmFormatDoc")!
        useContextMenuStore.getState().open(request, 1, 1)

        expect(await runContextMenuAction(request, command)).toBe("error")
        expect(calls).toContain("plugin:dialog|message")
        unregisterView(EDIT_PATH, view)
    })

    it("editor: 沒有 active view 時 cmCut/cmCopy/cmPaste/cmFormatDoc 都是 no-op、不 throw", () => {
        useWorkspaceStore.setState({ groups: [{ tabs: [], activePath: null }], activeGroupIndex: 0 })
        expect(() => runLegacyContextMenuAction("editor", "cmCopy", {})).not.toThrow()
        expect(() => runLegacyContextMenuAction("editor", "cmCut", {})).not.toThrow()
        expect(() => runLegacyContextMenuAction("editor", "cmPaste", {})).not.toThrow()
        expect(() => runLegacyContextMenuAction("editor", "cmFormatDoc", {})).not.toThrow()
    })

    it("git: cmFetch 呼叫 gitStore.runOp('fetch', …) → git_fetch_cmd", async () => {
        const calls: Array<{ cmd: string; args: unknown }> = []
        mockIPC((cmd, args) => {
            calls.push({ cmd, args })
            if (cmd === "git_branches") return { local: [], remote: [] }
            return cmd === "log_event" ? null : null
        })
        useGitStore.setState({ environment: { status: "ready", root: "/w", version: "2.50" } })
        await runLegacyContextMenuAction("git", "cmFetch", {})
        await vi.waitFor(() => expect(calls.find((call) => call.cmd === "git_fetch_cmd")?.args)
            .toMatchObject({ background: false, repositoryRoot: "/w" }))
        await vi.waitFor(() => expect(useGitStore.getState().busy).toBeNull())
    })

    it("status: cmPull 呼叫 gitStore.runOp('pull', …) → git_pull_cmd（git／status 共用同一段 dispatch）", async () => {
        const calls: Array<{ cmd: string; args: unknown }> = []
        mockIPC((cmd, args) => {
            calls.push({ cmd, args })
            if (cmd === "git_branches") return { local: [], remote: [] }
            return cmd === "log_event" ? null : null
        })
        useGitStore.setState({ environment: { status: "ready", root: "/w", version: "2.50" } })
        await runLegacyContextMenuAction("status", "cmPull", {})
        await vi.waitFor(() => expect(calls.find((call) => call.cmd === "git_pull_cmd")?.args)
            .toMatchObject({ repositoryRoot: "/w" }))
        await vi.waitFor(() => expect(useGitStore.getState().busy).toBeNull())
    })

    it("git: cmPush 呼叫 gitStore.runOp('push', …) → git_push_cmd", async () => {
        const calls: Array<{ cmd: string; args: unknown }> = []
        mockIPC((cmd, args) => {
            calls.push({ cmd, args })
            if (cmd === "git_branches") return { local: [], remote: [] }
            return cmd === "log_event" ? null : null
        })
        useGitStore.setState({ environment: { status: "ready", root: "/w", version: "2.50" } })
        await runLegacyContextMenuAction("git", "cmPush", {})
        await vi.waitFor(() => expect(calls.find((call) => call.cmd === "git_push_cmd")?.args)
            .toMatchObject({ repositoryRoot: "/w" }))
        await vi.waitFor(() => expect(useGitStore.getState().busy).toBeNull())
    })

    it("git/status: cmCopyBranch／cmCopyHash 複製 requested repository 的目前分支與 HEAD oid", async () => {
        const calls: Array<{ cmd: string; args: unknown }> = []
        mockIPC((cmd, args) => {
            calls.push({ cmd, args })
            return cmd === "log_event" ? null : undefined
        })
        useGitStore.setState({
            environment: { status: "ready", root: "/w", version: "2.50" },
            status: makeStatus({ branch: "feature/x", headOid: "a".repeat(40) })
        })

        await runLegacyContextMenuAction("git", "cmCopyBranch", {})
        expect(
            (calls.find((c) => c.cmd === "plugin:clipboard-manager|write_text")?.args as { text: string }).text
        ).toBe("feature/x")

        calls.length = 0
        await runLegacyContextMenuAction("status", "cmCopyHash", {})
        expect(
            (calls.find((c) => c.cmd === "plugin:clipboard-manager|write_text")?.args as { text: string }).text
        ).toBe("a".repeat(40))

        calls.length = 0
        const staleRequest: ContextMenuRequest = { kind: "status", repositoryRoot: "/other" }
        expect(await executeLegacyContextMenuAction(staleRequest, "cmCopyHash")).toBe("cancelled")
        expect(calls.some((c) => c.cmd === "plugin:clipboard-manager|write_text")).toBe(false)
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
        runLegacyContextMenuAction("sshhost", "cmOpenSftp", { hostId: host.id })
        expect(useSftpStore.getState().activeTab).toBe("sftp")
        expect(useSshStore.getState().activeHostId).toBe(host.id)
        // Password host → begins the connect flow rather than silently no-op'ing.
        expect(useSshStore.getState().pendingAuthHostId).toBe(host.id)
    })

    it("sshhost: Disconnect backend failure enters shared action feedback and preserves the live session", async () => {
        const calls: string[] = []
        mockIPC((cmd) => {
            calls.push(cmd)
            if (cmd === "ssh_disconnect") throw new Error("disconnect failed")
            return cmd === "log_event" ? null : undefined
        })
        const request: ContextMenuRequest = {
            kind: "sshhost",
            hostId: "clicked",
            address: "clicked@example.com:22"
        }
        useSshStore.setState({
            hosts: [{
                id: "clicked",
                name: "Clicked",
                host: "example.com",
                port: 22,
                user: "clicked",
                authKind: "password"
            }],
            sessions: {
                clicked: {
                    hostId: "clicked",
                    sessionId: "session-clicked",
                    status: "connected",
                    fingerprint: null,
                    knownHost: true,
                    error: null
                }
            },
            activeHostId: "unrelated-active-host",
            pendingAuthHostId: null
        })
        const command = commandFor(request, "cmDisconnect")
        if (!command) throw new Error("missing cmDisconnect")
        useContextMenuStore.getState().open(request, 1, 1)

        expect(await runContextMenuAction(request, command)).toBe("error")
        expect(calls).toContain("plugin:dialog|message")
        expect(useSshStore.getState().sessions.clicked).toMatchObject({
            status: "connected",
            sessionId: "session-clicked"
        })
    })

    it("dbconn: cmDisconnect 呼叫 dbStore.disconnect — 保留描述子、標記 disconnected", async () => {
        useDbStore.setState({
            connections: [
                { connId: "c1", connectionGeneration: "generation-1" as never, kind: "sqlite", name: "a", descriptorId: "d1", targetKey: "/a.db", title: "/a.db" }
            ],
            saved: [{ id: "d1", configGeneration: 1, targetKey: "/a.db", kind: "sqlite", name: "a", path: "/a.db" }],
            sessions: { d1: { descriptorId: "d1", connId: "c1", status: "connected", error: null } },
            activeDescriptorId: "d1",
            activeConnId: "c1",
            tables: {},
            queries: {}
        })
        runLegacyContextMenuAction("dbconn", "cmDisconnect", { descriptorId: "d1" })
        await vi.waitFor(() => {
            expect(useDbStore.getState().connections).toEqual([])
        })
        expect(useDbStore.getState().sessions.d1).toMatchObject({ status: "disconnected", connId: null })
        // The saved descriptor survives so the row can reconnect.
        expect(useDbStore.getState().saved.map((x) => x.id)).toEqual(["d1"])
    })

    it("dbconn: cmDisconnect failure preserves the live connection and reaches action feedback", async () => {
        const calls: string[] = []
        mockIPC((cmd) => {
            calls.push(cmd)
            if (cmd === "db_profile_disconnect") {
                throw { code: "connectionFailed", message: "database close failed" }
            }
            if (cmd === "plugin:dialog|message") return "Ok"
            return null
        })
        useDbStore.setState({
            connections: [
                { connId: "c1", connectionGeneration: "generation-1" as never, kind: "sqlite", name: "a", descriptorId: "d1", targetKey: "/a.db", title: "/a.db" }
            ],
            saved: [{ id: "d1", configGeneration: 1, targetKey: "/a.db", kind: "sqlite", name: "a", path: "/a.db" }],
            sessions: { d1: { descriptorId: "d1", connId: "c1", status: "connected", error: null } },
            activeDescriptorId: "d1",
            activeConnId: "c1",
            tables: { c1: [] },
            queries: {}
        })
        const request: ContextMenuRequest = { kind: "dbconn", descriptorId: "d1", address: "/a.db" }
        const command = commandFor(request, "cmDisconnect")
        if (!command) throw new Error("missing cmDisconnect")
        useContextMenuStore.getState().open(request, 1, 1)

        expect(await runContextMenuAction(request, command)).toBe("error")
        expect(useDbStore.getState().connections.map((connection) => connection.connId)).toEqual(["c1"])
        expect(useDbStore.getState().sessions.d1).toMatchObject({
            status: "error",
            connId: "c1",
            error: "connectionFailed"
        })
        expect(calls).toContain("plugin:dialog|message")
        expect(calls).not.toContain("log_event")
    })

    it("dbconn: cmOpenDb 已移出 legacy dispatcher，必須由 typed registry 執行", async () => {
        useDbStore.setState({
            connections: [
                { connId: "c1", kind: "sqlite", name: "a", descriptorId: "d1", targetKey: "/a.db", title: "/a.db" }
            ],
            saved: [{ id: "d1", targetKey: "/a.db", kind: "sqlite", name: "a", path: "/a.db" }],
            sessions: {},
            activeConnId: null,
            tables: { c1: [] },
            queries: {}
        })
        expect(await runLegacyContextMenuAction("dbconn", "cmOpenDb", { descriptorId: "d1" }))
            .toBe("cancelled")
        expect(useDbStore.getState().activeConnId).toBeNull()
    })

    it("dbconn: SQLite Open failure keeps row error and reaches shared actionFeedback", async () => {
        const calls: string[] = []
        mockIPC((cmd) => {
            calls.push(cmd)
            if (cmd === "db_profile_open") {
                throw { code: "connectionFailed", message: "RAW FILE DETAIL" }
            }
            if (cmd === "plugin:dialog|message") return "Ok"
            return null
        })
        useDbStore.setState({
            connections: [],
            saved: [{ id: "d1", targetKey: "/a.db", kind: "sqlite", name: "a", path: "/a.db" }],
            sessions: {},
            activeConnId: null,
            tables: {},
            queries: {},
            reconnectRequest: null
        })
        const request: ContextMenuRequest = {
            kind: "dbconn",
            descriptorId: "d1",
            address: "/a.db"
        }
        const command = commandFor(request, "cmOpenDb")
        if (!command) throw new Error("missing cmOpenDb")

        expect(await runContextMenuAction(request, command)).toBe("error")
        expect(useDbStore.getState().sessions.d1).toEqual({
            descriptorId: "d1",
            connId: null,
            status: "error",
            error: "connectionFailed"
        })
        expect(calls).toContain("plugin:dialog|message")
    })

    it("dbconn: cmCopyAddr 依 descriptorId 重讀目前位址，不信任 stale payload", async () => {
        const calls: Array<{ cmd: string; args: unknown }> = []
        mockIPC((cmd, args) => {
            calls.push({ cmd, args })
            return cmd === "log_event" ? null : undefined
        })
        useDbStore.setState({
            saved: [{
                id: "d1",
                targetKey: "postgres:current.example:5432:app",
                kind: "postgres",
                name: "app",
                host: "current.example",
                port: 5432,
                database: "app",
                user: "admin",
                ssl: false
            }]
        })
        runLegacyContextMenuAction("dbconn", "cmCopyAddr", {
            descriptorId: "d1",
            addr: "stale@old.example:1/old"
        })
        await vi.waitFor(() => {
            expect(calls.some((c) => c.cmd === "plugin:clipboard-manager|write_text")).toBe(true)
        })
        expect(
            (calls.find((c) => c.cmd === "plugin:clipboard-manager|write_text")?.args as { text: string }).text
        ).toBe("admin@current.example:5432/app")
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

        runLegacyContextMenuAction("explorer", "cmNewFile", {})
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

        runLegacyContextMenuAction("explorer", "cmNewFolder", {})
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

        runLegacyContextMenuAction("explorer", "cmNewFile", {})
        await new Promise((resolve) => setTimeout(resolve, 0))
        expect(calls.some((c) => c.cmd === "fs_create_file")).toBe(false)
        expect(useWorkspaceStore.getState().treeRevision).toBe(0)
        promptSpy.mockRestore()
    })

    it("file: cmRename prompt 新名稱 → fs_rename（同目錄換 basename），成功後 refreshTree", async () => {
        const calls = ipcCalls()
        useWorkspaceStore.setState({ workspacePath: "/w", treeRevision: 0 })
        const promptSpy = vi.spyOn(window, "prompt").mockReturnValue("renamed.ts")

        runLegacyContextMenuAction("file", "cmRename", { path: "/w/src/old.ts" })
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

        runLegacyContextMenuAction("file", "cmRename", { path: "/w/src/old.ts" })
        await new Promise((resolve) => setTimeout(resolve, 0))
        expect(calls.some((c) => c.cmd === "fs_rename")).toBe(false)
        promptSpy.mockRestore()
    })

    it("file: cmRename 的 Windows prompt 預設值只顯示 basename", async () => {
        const workspace = String.raw`\\?\C:\Work\中文 workspace`
        const path = String.raw`\\?\C:\Work\中文 workspace\old.ts`
        useWorkspaceStore.setState({ workspacePath: workspace, treeRevision: 0 })
        const promptSpy = vi.spyOn(window, "prompt").mockReturnValue(null)

        runLegacyContextMenuAction("file", "cmRename", { path })

        await vi.waitFor(() => expect(promptSpy).toHaveBeenCalled())
        expect(promptSpy).toHaveBeenCalledWith(expect.any(String), "old.ts")
        promptSpy.mockRestore()
    })

    it("file: cmDelete 檔案 → confirm（文字含檔名、不含資料夾警告）→ fs_delete + refreshTree", async () => {
        const calls = ipcCalls()
        useWorkspaceStore.setState({ workspacePath: "/w", treeRevision: 0 })

        runLegacyContextMenuAction("file", "cmDelete", { path: "/w/f.ts", isDir: false })
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

    it("file: cmDelete 資料夾 → localized confirm 明確涵蓋全部內容", async () => {
        const calls = ipcCalls()
        useWorkspaceStore.setState({ workspacePath: "/w", treeRevision: 0 })

        runLegacyContextMenuAction("file", "cmDelete", { path: "/w/dir", isDir: true })
        await vi.waitFor(() => expect(calls.some((c) => c.cmd === "fs_delete")).toBe(true))
        const confirmCall = calls.find((c) => c.cmd === "plugin:dialog|message")
        expect(String(confirmCall?.args.message)).toContain("dir")
        expect(String(confirmCall?.args.message)).toContain("all of its contents")
    })

    it("file: cmDelete 的 Windows confirm 顯示 basename，後端仍收到 raw path", async () => {
        const workspace = String.raw`\\?\C:\Work\中文 workspace`
        const path = String.raw`\\?\C:\Work\中文 workspace\a.ts`
        const calls: Array<{ cmd: string; args: Record<string, unknown> }> = []
        mockIPC((cmd, args) => {
            calls.push({ cmd, args: (args ?? {}) as Record<string, unknown> })
            if (cmd === "plugin:dialog|message") return "Ok"
            return cmd === "log_event" ? null : undefined
        })
        useWorkspaceStore.setState({ workspacePath: workspace, treeRevision: 0 })

        runLegacyContextMenuAction("file", "cmDelete", { path, isDir: false })

        await vi.waitFor(() => expect(calls.some((call) => call.cmd === "fs_delete")).toBe(true))
        const message = String(calls.find((call) => call.cmd === "plugin:dialog|message")?.args.message)
        expect(message).toContain("a.ts")
        expect(message).not.toContain(path)
        expect(calls.find((call) => call.cmd === "fs_delete")?.args).toMatchObject({ workspace, path })
    })

    it("file: cmDelete 取消 confirm 不呼叫 fs_delete、不 refreshTree", async () => {
        const calls: Array<{ cmd: string }> = []
        mockIPC((cmd) => {
            calls.push({ cmd })
            if (cmd === "plugin:dialog|message") return "Cancel"
            return cmd === "log_event" ? null : undefined
        })
        useWorkspaceStore.setState({ workspacePath: "/w", treeRevision: 0 })

        runLegacyContextMenuAction("file", "cmDelete", { path: "/w/f.ts", isDir: false })
        await new Promise((resolve) => setTimeout(resolve, 0))
        expect(calls.some((c) => c.cmd === "fs_delete")).toBe(false)
        expect(useWorkspaceStore.getState().treeRevision).toBe(0)
    })

    it("file: cmReveal 呼叫 opener 的 reveal_item_in_dir（傳目標 path）", async () => {
        const calls = ipcCalls()
        runLegacyContextMenuAction("file", "cmReveal", { path: "/w/f.ts" })
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

        runLegacyContextMenuAction("file", "cmOpenInBrowser", { path: "/w/site/index.html" })

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
        runLegacyContextMenuAction("file", "cmDelete", { path: "/w/f.ts", isDir: false })
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
        runLegacyContextMenuAction("file", "cmDelete", { path: "/w/dir", isDir: true })
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
        runLegacyContextMenuAction("file", "cmRename", { path: "/w/src/old.ts" })
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

    // SVG preview 的 close→forget／rename 遷移契約不只走 TabBar 的關閉鈕：
    // context-menu 的關閉、刪除、重新命名三條路徑也必須維持同一語意
    // （store 記「明確關閉」，預設開啟——SvgSplitView plan t3-3b）。
    it("tab: cmCloseTab 關閉 SVG tab 時清除其 preview 關閉狀態（重開回到預設開啟）", async () => {
        useWorkspaceStore.setState({
            workspacePath: "/w",
            activeGroupIndex: 0,
            groups: [
                {
                    activePath: "/w/logo.svg",
                    tabs: [
                        { path: "/w/a.ts", name: "a.ts", dirty: false, externallyModified: false },
                        { path: "/w/logo.svg", name: "logo.svg", dirty: false, externallyModified: false }
                    ]
                }
            ]
        })
        useSvgPreviewStore.getState().toggle("/w/logo.svg")
        expect(useSvgPreviewStore.getState().isOpen("/w/logo.svg")).toBe(false)

        runLegacyContextMenuAction("tab", "cmCloseTab", { path: "/w/logo.svg", groupIndex: 0 })
        await vi.waitFor(() => {
            expect(
                useWorkspaceStore.getState().groups[0].tabs.some((t) => t.path === "/w/logo.svg")
            ).toBe(false)
        })
        expect(useSvgPreviewStore.getState().isOpen("/w/logo.svg")).toBe(true)
    })

    it("file: cmDelete SVG → 清除其 preview 關閉狀態（不殘留 closedPaths）", async () => {
        ipcCalls()
        useWorkspaceStore.setState({
            workspacePath: "/w",
            treeRevision: 0,
            activeGroupIndex: 0,
            groups: [
                {
                    activePath: "/w/logo.svg",
                    tabs: [
                        { path: "/w/logo.svg", name: "logo.svg", dirty: false, externallyModified: false }
                    ]
                }
            ]
        })
        useSvgPreviewStore.getState().toggle("/w/logo.svg")

        runLegacyContextMenuAction("file", "cmDelete", { path: "/w/logo.svg", isDir: false })
        await vi.waitFor(() => {
            expect(useWorkspaceStore.getState().groups[0].tabs).toEqual([])
        })
        expect(useSvgPreviewStore.getState().isOpen("/w/logo.svg")).toBe(true)
    })

    it("file: cmRename SVG → 使用者的 preview 關閉狀態遷移到新路徑、舊路徑不殘留", async () => {
        ipcCalls()
        useWorkspaceStore.setState({
            workspacePath: "/w",
            treeRevision: 0,
            activeGroupIndex: 0,
            groups: [
                {
                    activePath: "/w/old.svg",
                    tabs: [
                        { path: "/w/old.svg", name: "old.svg", dirty: false, externallyModified: false }
                    ]
                }
            ]
        })
        useSvgPreviewStore.getState().toggle("/w/old.svg")
        const promptSpy = vi.spyOn(window, "prompt").mockReturnValue("new.svg")

        runLegacyContextMenuAction("file", "cmRename", { path: "/w/old.svg" })
        await vi.waitFor(() => {
            expect(
                useWorkspaceStore.getState().groups[0].tabs.some((t) => t.path === "/w/new.svg")
            ).toBe(true)
        })
        expect(useSvgPreviewStore.getState().isOpen("/w/new.svg")).toBe(false)
        expect(useSvgPreviewStore.getState().closedPaths["/w/old.svg"]).toBeUndefined()
        promptSpy.mockRestore()
    })

    it("file: cmRename SVG（toggle 兩次＝回到開啟）→ 舊路徑的 false 旗標不殘留、新路徑維持預設開啟", async () => {
        ipcCalls()
        useWorkspaceStore.setState({
            workspacePath: "/w",
            treeRevision: 0,
            activeGroupIndex: 0,
            groups: [
                {
                    activePath: "/w/old.svg",
                    tabs: [
                        { path: "/w/old.svg", name: "old.svg", dirty: false, externallyModified: false }
                    ]
                }
            ]
        })
        useSvgPreviewStore.getState().toggle("/w/old.svg")
        useSvgPreviewStore.getState().toggle("/w/old.svg")
        expect(useSvgPreviewStore.getState().closedPaths["/w/old.svg"]).toBe(false)
        const promptSpy = vi.spyOn(window, "prompt").mockReturnValue("new.svg")

        runLegacyContextMenuAction("file", "cmRename", { path: "/w/old.svg" })
        await vi.waitFor(() => {
            expect(
                useWorkspaceStore.getState().groups[0].tabs.some((t) => t.path === "/w/new.svg")
            ).toBe(true)
        })
        expect(useSvgPreviewStore.getState().closedPaths["/w/old.svg"]).toBeUndefined()
        expect(useSvgPreviewStore.getState().isOpen("/w/new.svg")).toBe(true)
        expect(useSvgPreviewStore.getState().closedPaths["/w/new.svg"]).toBeUndefined()
        promptSpy.mockRestore()
    })
})

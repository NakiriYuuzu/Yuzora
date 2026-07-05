import { beforeEach, describe, expect, it, vi } from "vitest"
import { clearMocks, mockIPC } from "@tauri-apps/api/mocks"
import type { MouseEvent as ReactMouseEvent } from "react"

import {
    contextMenuHandler,
    runContextMenuAction,
    suppressContextMenu,
    useContextMenuStore
} from "@/state/contextMenuStore"
import { useDiffModalStore } from "@/state/diffModalStore"
import { useGitStore } from "@/state/gitStore"
import { useUiStore, uiInitialState } from "@/state/uiStore"
import { useWorkspaceStore } from "@/state/workspaceStore"
import type { GitStatus } from "@/lib/types"

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
    useContextMenuStore.setState({ kind: null, x: 0, y: 0, payload: {} })
    useDiffModalStore.setState({ open: false, source: null, activeIndex: 0, mode: "unified" })
    useUiStore.setState(uiInitialState)
    useWorkspaceStore.setState({ groups: [{ tabs: [], activePath: null }], activeGroupIndex: 0 })
    useGitStore.setState({ status: null, environment: null })
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

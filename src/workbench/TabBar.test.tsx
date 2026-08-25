import { expect, test, afterEach, beforeEach, vi } from "vitest"
import { act, render, screen, fireEvent, waitFor } from "@testing-library/react"
import { mockIPC, clearMocks } from "@tauri-apps/api/mocks"

// The dirty-close flow now routes through confirmDialogStore's imperative gate
// (replacing the Tauri native confirm()). Mock the store so tests drive the
// decision, mock saveDirtyTab so the "save" branch doesn't touch the editor/ipc,
// and mock @tauri-apps/plugin-dialog so we can assert native confirm is gone.
// vi.hoisted because these are referenced inside the hoisted vi.mock factories
// (plugin-dialog loads very early via contextMenuStore, before plain consts init).
const { requestUnsavedDecision, nativeConfirm, nativeMessage, herdrTabRename, herdrTabMove } = vi.hoisted(() => ({
    requestUnsavedDecision: vi.fn(),
    nativeConfirm: vi.fn(),
    nativeMessage: vi.fn(),
    herdrTabRename: vi.fn(),
    herdrTabMove: vi.fn()
}))
vi.mock("../state/confirmDialogStore", () => ({
    useConfirmDialogStore: { getState: () => ({ requestUnsavedDecision }) }
}))
vi.mock("@tauri-apps/plugin-dialog", () => ({ confirm: nativeConfirm, message: nativeMessage }))
vi.mock("../editor/saveDocument", () => ({ saveDirtyTab: vi.fn() }))
vi.mock("@/lib/herdrIpc", async (importOriginal) => ({
    ...(await importOriginal<typeof import("@/lib/herdrIpc")>()),
    herdrTabRename,
    herdrTabMove
}))

import { TabBar } from "./TabBar"
import { PREVIEW_TAB_PATH, useWorkspaceStore } from "../state/workspaceStore"
import { useAppDialogStore } from "../state/appDialogStore"
import { useContextMenuStore } from "../state/contextMenuStore"
import { useHerdrStore } from "../state/herdrStore"
import { useTextInputDialogStore } from "../state/textInputDialogStore"
import { useSvgPreviewStore } from "../state/svgPreviewStore"
import { useUiStore, uiInitialState } from "../state/uiStore"
import { isMarkdownPreviewTab } from "../lib/markdownPreviewTab"
import { saveDirtyTab } from "../editor/saveDocument"

const initialHerdrState = useHerdrStore.getState()

beforeEach(() => {
    requestUnsavedDecision.mockReset()
    nativeConfirm.mockReset()
    nativeMessage.mockReset()
    useAppDialogStore.setState({ pending: null })
    herdrTabRename.mockReset().mockResolvedValue(undefined)
    herdrTabMove.mockReset().mockResolvedValue(undefined)
    vi.mocked(saveDirtyTab).mockReset().mockResolvedValue({ kind: "saved" })
})

afterEach(() => {
    clearMocks()
    useContextMenuStore.setState({ request: null, x: 0, y: 0, availabilityRevision: 0 })
    useUiStore.setState(uiInitialState)

    useSvgPreviewStore.getState().reset()
    useTextInputDialogStore.setState({ pending: null })
    useAppDialogStore.setState({ pending: null })
    useHerdrStore.setState(initialHerdrState, true)
})

function seedTabs() {
    useWorkspaceStore.setState({
        workspacePath: "/w",
        activeGroupIndex: 0,
        groups: [
            {
                activePath: "/w/a.ts",
                tabs: [
                    { path: "/w/a.ts", name: "a.ts", dirty: false, externallyModified: false },
                    { path: "/w/b.ts", name: "b.ts", dirty: true, externallyModified: false }
                ]
            }
        ]
    })
}

test("點擊 tab 切換 active", () => {
    mockIPC((cmd) => (cmd === "log_event" ? null : undefined))
    seedTabs()
    render(<TabBar groupIndex={0} />)
    fireEvent.click(screen.getByText("b.ts"))
    expect(useWorkspaceStore.getState().groups[0].activePath).toBe("/w/b.ts")
})

test("點擊 tab icon 也會切換 active", () => {
    mockIPC((cmd) => (cmd === "log_event" ? null : undefined))
    seedTabs()
    render(<TabBar groupIndex={0} />)
    const tab = screen.getByText("b.ts").closest(".tab")
    const icon = tab?.querySelector("img")
    expect(icon).toBeTruthy()

    fireEvent.click(icon!)

    expect(useWorkspaceStore.getState().groups[0].activePath).toBe("/w/b.ts")
})

test("dirty tab 顯示標記", () => {
    seedTabs()
    render(<TabBar groupIndex={0} />)
    expect(screen.getByText("b.ts").closest(".tab")?.querySelector(".dirty-dot")).toBeTruthy()
})

test("dirty tab 關閉走新 modal：cancel → tab 仍在、不呼叫 native confirm", async () => {
    mockIPC((cmd) => (cmd === "log_event" ? null : undefined))
    requestUnsavedDecision.mockResolvedValue("cancel")
    seedTabs()
    render(<TabBar groupIndex={0} />)
    fireEvent.click(screen.getByLabelText("Close b.ts"))
    await waitFor(() => expect(requestUnsavedDecision).toHaveBeenCalled())
    expect(
        useWorkspaceStore.getState().groups[0].tabs.some((t) => t.path === "/w/b.ts")
    ).toBe(true)
    expect(saveDirtyTab).not.toHaveBeenCalled()
    expect(nativeConfirm).not.toHaveBeenCalled()
})

test("dirty tab 關閉走新 modal：discard → tab 被關、不存檔", async () => {
    mockIPC((cmd) => (cmd === "log_event" ? null : undefined))
    requestUnsavedDecision.mockResolvedValue("discard")
    seedTabs()
    render(<TabBar groupIndex={0} />)
    fireEvent.click(screen.getByLabelText("Close b.ts"))
    await waitFor(() =>
        expect(
            useWorkspaceStore.getState().groups[0].tabs.some((t) => t.path === "/w/b.ts")
        ).toBe(false)
    )
    expect(saveDirtyTab).not.toHaveBeenCalled()
    expect(nativeConfirm).not.toHaveBeenCalled()
})

test("dirty tab 關閉走新 modal：save → 先 saveDirtyTab 再關", async () => {
    mockIPC((cmd) => (cmd === "log_event" ? null : undefined))
    requestUnsavedDecision.mockResolvedValue("save")
    seedTabs()
    render(<TabBar groupIndex={0} />)
    fireEvent.click(screen.getByLabelText("Close b.ts"))
    await waitFor(() => expect(saveDirtyTab).toHaveBeenCalledWith("/w/b.ts"))
    expect(
        useWorkspaceStore.getState().groups[0].tabs.some((t) => t.path === "/w/b.ts")
    ).toBe(false)
    expect(nativeConfirm).not.toHaveBeenCalled()
})

test("dirty Mixed tab 儲存被 block 時不關閉 tab", async () => {
    mockIPC((cmd) => (cmd === "log_event" ? null : undefined))
    requestUnsavedDecision.mockResolvedValue("save")
    vi.mocked(saveDirtyTab).mockResolvedValue({ kind: "blocked", reason: "mixed" })
    seedTabs()
    render(<TabBar groupIndex={0} />)

    fireEvent.click(screen.getByLabelText("Close b.ts"))

    await waitFor(() => expect(saveDirtyTab).toHaveBeenCalledWith("/w/b.ts"))
    expect(
        useWorkspaceStore.getState().groups[0].tabs.some((t) => t.path === "/w/b.ts")
    ).toBe(true)
})

test("dirty tab 儲存 I/O failed 時不關閉 tab", async () => {
    mockIPC((cmd) => (cmd === "log_event" ? null : undefined))
    requestUnsavedDecision.mockResolvedValue("save")
    vi.mocked(saveDirtyTab).mockResolvedValue({ kind: "failed" })
    seedTabs()
    render(<TabBar groupIndex={0} />)

    fireEvent.click(screen.getByLabelText("Close b.ts"))

    await waitFor(() => expect(saveDirtyTab).toHaveBeenCalledWith("/w/b.ts"))
    expect(
        useWorkspaceStore.getState().groups[0].tabs.some((t) => t.path === "/w/b.ts")
    ).toBe(true)
})

test("externallyModified tab 點 ⟳ 主動開啟解決器（spec 入口 b）", () => {
    useWorkspaceStore.setState({
        workspacePath: "/w",
        activeGroupIndex: 0,
        groups: [
            {
                activePath: "/w/a.ts",
                tabs: [{ path: "/w/a.ts", name: "a.ts", dirty: false, externallyModified: true }]
            }
        ]
    })
    render(<TabBar groupIndex={0} />)
    fireEvent.click(screen.getByRole("button", { name: "Resolve external changes a.ts" }))
    expect(useUiStore.getState().resolverPath).toBe("/w/a.ts")
})

function seedMdTab() {
    useWorkspaceStore.setState({
        workspacePath: "/w",
        activeGroupIndex: 0,
        groups: [
            {
                activePath: "/w/r.md",
                tabs: [
                    { path: "/w/r.md", name: "r.md", dirty: false, externallyModified: false },
                    { path: "/w/a.ts", name: "a.ts", dirty: false, externallyModified: false }
                ]
            }
        ]
    })
}

test(".md 分頁顯示 preview toggle、非 .md 不顯示", () => {
    mockIPC((cmd) => (cmd === "open_file" ? { kind: "full", content: "", size: 0 } : undefined))
    seedMdTab()
    render(<TabBar groupIndex={0} />)
    expect(screen.queryByLabelText("Toggle preview r.md")).toBeTruthy()
    expect(screen.queryByLabelText("Toggle preview a.ts")).toBeNull()
})

test("點 preview toggle 切換開啟狀態", () => {
    mockIPC((cmd) => {
        if (cmd === "log_event") return null
        if (cmd === "open_file") return { kind: "full", content: "", size: 0 }
        return undefined
    })
    seedMdTab()
    render(<TabBar groupIndex={0} />)
    expect(useWorkspaceStore.getState().hasMarkdownPreview("/w/r.md")).toBe(false)
    fireEvent.click(screen.getByLabelText("Toggle preview r.md"))
    expect(useWorkspaceStore.getState().hasMarkdownPreview("/w/r.md")).toBe(true)
    expect(useWorkspaceStore.getState().groups[1].tabs.some(isMarkdownPreviewTab)).toBe(true)
    fireEvent.click(screen.getByLabelText("Toggle preview r.md"))
    expect(useWorkspaceStore.getState().hasMarkdownPreview("/w/r.md")).toBe(false)
})

test("TabBar 只管理 toggle，不再 mount Markdown preview", () => {
    seedMdTab()
    useWorkspaceStore.getState().toggleMarkdownPreview("/w/r.md", 0)
    render(<TabBar groupIndex={0} />)
    expect(screen.queryByRole("complementary", { name: "Markdown preview" })).toBeNull()
})

test("非 active md tab 按 preview toggle 會先設為 active", () => {
    mockIPC((cmd) => {
        if (cmd === "log_event") return null
        return undefined
    })
    useWorkspaceStore.setState({
        workspacePath: "/w",
        activeGroupIndex: 0,
        groups: [
            {
                activePath: "/w/a.md",
                tabs: [
                    { path: "/w/a.md", name: "a.md", dirty: false, externallyModified: false },
                    { path: "/w/b.md", name: "b.md", dirty: false, externallyModified: false }
                ]
            }
        ]
    })
    render(<TabBar groupIndex={0} />)
    const bEye = screen.getByLabelText("Toggle preview b.md")
    fireEvent.click(bEye)
    expect(bEye.getAttribute("aria-pressed")).toBe("true")
    expect(useWorkspaceStore.getState().groups[0].activePath).toBe("/w/b.md")
    expect(screen.queryByRole("complementary", { name: "Markdown preview" })).toBeNull()
})

test("關閉 .md 分頁時清除其 preview 開關狀態（W5）", async () => {
    mockIPC((cmd) => {
        if (cmd === "log_event") return null
        if (cmd === "open_file") return { kind: "full", content: "", size: 0 }
        return undefined
    })
    seedMdTab()
    useWorkspaceStore.getState().toggleMarkdownPreview("/w/r.md", 0)
    render(<TabBar groupIndex={0} />)
    fireEvent.click(screen.getByLabelText("Close r.md"))
    await waitFor(() =>
        expect(useWorkspaceStore.getState().hasMarkdownPreview("/w/r.md")).toBe(false)
    )
})

// SVG 分頁的 preview toggle 是「反相語意」：store 記明確關閉、預設開啟，
// 與 markdown（記開啟、預設關閉）相反——這裡固定住雙模式各自的行為。
function seedMixedPreviewTabs() {
    useWorkspaceStore.setState({
        workspacePath: "/w",
        activeGroupIndex: 0,
        groups: [
            {
                activePath: "/w/logo.svg",
                tabs: [
                    { path: "/w/logo.svg", name: "logo.svg", dirty: false, externallyModified: false },
                    { path: "/w/r.md", name: "r.md", dirty: false, externallyModified: false }
                ]
            }
        ]
    })
}

test("svg 分頁 toggle 預設 aria-pressed=true（反相語意），點擊後關閉並記錄", () => {
    mockIPC((cmd) => (cmd === "log_event" ? null : undefined))
    seedMixedPreviewTabs()
    render(<TabBar groupIndex={0} />)
    const svgEye = screen.getByLabelText("Toggle preview logo.svg")
    expect(svgEye.getAttribute("aria-pressed")).toBe("true")
    fireEvent.click(svgEye)
    expect(svgEye.getAttribute("aria-pressed")).toBe("false")
    expect(useSvgPreviewStore.getState().isOpen("/w/logo.svg")).toBe(false)
    fireEvent.click(svgEye)
    expect(useSvgPreviewStore.getState().isOpen("/w/logo.svg")).toBe(true)
})

test("md 與 svg 分頁並存：toggle 各自分流（title 與 aria-pressed 互不干擾）", () => {
    mockIPC((cmd) => (cmd === "log_event" ? null : undefined))
    seedMixedPreviewTabs()
    render(<TabBar groupIndex={0} />)
    const svgEye = screen.getByLabelText("Toggle preview logo.svg")
    const mdEye = screen.getByLabelText("Toggle preview r.md")
    expect(svgEye.getAttribute("title")).toBe("Toggle SVG preview")
    expect(mdEye.getAttribute("title")).toBe("Toggle Markdown preview")
    // 預設：svg 開（true）、md 關（false）。
    expect(svgEye.getAttribute("aria-pressed")).toBe("true")
    expect(mdEye.getAttribute("aria-pressed")).toBe("false")
    fireEvent.click(mdEye)
    expect(useWorkspaceStore.getState().hasMarkdownPreview("/w/r.md")).toBe(true)
    expect(useSvgPreviewStore.getState().isOpen("/w/logo.svg")).toBe(true)
    expect(useSvgPreviewStore.getState().closedPaths["/w/logo.svg"]).toBeUndefined()
})

test("關閉 svg 分頁清除其明確關閉狀態（重開回到預設開啟）", async () => {
    mockIPC((cmd) => (cmd === "log_event" ? null : undefined))
    seedMixedPreviewTabs()
    render(<TabBar groupIndex={0} />)
    fireEvent.click(screen.getByLabelText("Toggle preview logo.svg"))
    expect(useSvgPreviewStore.getState().isOpen("/w/logo.svg")).toBe(false)
    fireEvent.click(screen.getByLabelText("Close logo.svg"))
    await waitFor(() =>
        expect(
            useWorkspaceStore.getState().groups[0].tabs.some((t) => t.path === "/w/logo.svg")
        ).toBe(false)
    )
    expect(useSvgPreviewStore.getState().isOpen("/w/logo.svg")).toBe(true)
})

function seedPreviewTab() {
    useWorkspaceStore.setState({
        workspacePath: "/w",
        activeGroupIndex: 0,
        groups: [
            {
                activePath: PREVIEW_TAB_PATH,
                tabs: [
                    { path: "/w/a.ts", name: "a.ts", dirty: false, externallyModified: false },
                    {
                        path: PREVIEW_TAB_PATH,
                        name: "Preview",
                        dirty: false,
                        externallyModified: false,
                        kind: "preview"
                    }
                ]
            }
        ]
    })
}

test("preview 分頁渲染標籤、無 dirty 點、無 markdown preview toggle", () => {
    seedPreviewTab()
    render(<TabBar groupIndex={0} />)
    const previewTab = screen.getByText("Preview").closest(".tab")
    expect(previewTab).toBeTruthy()
    expect(previewTab?.querySelector(".dirty-dot")).toBeNull()
    expect(screen.queryByLabelText("Toggle preview Preview")).toBeNull()
})

test("關閉 preview 分頁走 closePreviewTab（無 confirm、singleton 移除）", async () => {
    mockIPC((cmd) => (cmd === "log_event" ? null : undefined))
    seedPreviewTab()
    render(<TabBar groupIndex={0} />)
    fireEvent.click(screen.getByLabelText("Close Preview"))
    await waitFor(() =>
        expect(
            useWorkspaceStore.getState().groups[0].tabs.some((t) => t.path === PREVIEW_TAB_PATH)
        ).toBe(false)
    )
    // 檔案分頁保留並回補為 active。
    expect(useWorkspaceStore.getState().groups[0].tabs.some((t) => t.path === "/w/a.ts")).toBe(true)
    expect(useWorkspaceStore.getState().groups[0].activePath).toBe("/w/a.ts")
})

test("關閉 markdown preview tab 無 dirty prompt 並移除 preview-only group", async () => {
    mockIPC((cmd) => (cmd === "log_event" ? null : undefined))
    seedMdTab()
    useWorkspaceStore.getState().toggleMarkdownPreview("/w/r.md", 0)
    render(<TabBar groupIndex={1} />)
    fireEvent.click(screen.getByLabelText("Close Preview"))
    await waitFor(() => expect(useWorkspaceStore.getState().groups).toHaveLength(1))
    expect(requestUnsavedDecision).not.toHaveBeenCalled()
    expect(useWorkspaceStore.getState().hasMarkdownPreview("/w/r.md")).toBe(false)
})

test("右鍵 tab 開啟 tab 選單並帶 path 與 groupIndex", () => {
    seedTabs()
    render(<TabBar groupIndex={0} />)
    fireEvent.contextMenu(screen.getByText("b.ts"))
    expect(useContextMenuStore.getState().request).toMatchObject({
        kind: "tab",
        workspacePath: "/w",
        path: "/w/b.ts",
        groupIndex: 0
    })
})

test("closing a Herdr tab waits for runtime tab.close before removing the local page", async () => {
    let finishRuntimeClose!: () => void
    const runtimeClose = new Promise<void>((resolve) => {
        finishRuntimeClose = resolve
    })
    const closeRequests: Array<Record<string, unknown>> = []
    mockIPC((cmd, args) => {
        if (cmd === "herdr_tab_close") {
            closeRequests.push(args as Record<string, unknown>)
            return runtimeClose
        }
        if (cmd === "log_event") return null
        return undefined
    })
    const pagePath = "yuzora://herdr/default/term-close"
    useWorkspaceStore.setState({
        workspacePath: "/w",
        activeGroupIndex: 0,
        groups: [{
            activePath: pagePath,
            tabs: [{
                path: pagePath,
                name: "Closable",
                dirty: false,
                externallyModified: false,
                kind: "herdr-terminal",
                herdrSessionId: "default",
                terminalId: "term-close",
                herdrTabId: "tab-close",
                paneId: "pane-close"
            }]
        }]
    })
    useHerdrStore.setState({
        sessions: [{
            name: "default",
            default: true,
            running: true,
            sessionDir: "/tmp/default",
            socketPath: "/tmp/default.sock"
        }],
        selectedSessionName: "default"
    })

    render(<TabBar groupIndex={0} />)
    fireEvent.click(screen.getByLabelText("Close Closable"))

    await waitFor(() => expect(useAppDialogStore.getState().pending?.type).toBe("confirm"))
    useAppDialogStore.getState().respond(true)
    await waitFor(() => expect(closeRequests).toEqual([{ sessionName: "default", tabId: "tab-close" }]))
    expect(useWorkspaceStore.getState().groups[0].tabs).toHaveLength(1)

    finishRuntimeClose()
    await waitFor(() => expect(useWorkspaceStore.getState().groups[0].tabs).toHaveLength(0))
})

test("closing a moved Herdr page removes it by path after runtime close succeeds", async () => {
    let finishRuntimeClose!: () => void
    mockIPC((cmd) => {
        if (cmd === "herdr_tab_close") {
            return new Promise<void>((resolve) => {
                finishRuntimeClose = resolve
            })
        }
        if (cmd === "log_event") return null
        return undefined
    })
    const pagePath = "yuzora://herdr/default/term-close"
    const tab = {
        path: pagePath,
        name: "Closable",
        dirty: false,
        externallyModified: false,
        kind: "herdr-terminal" as const,
        herdrSessionId: "default",
        terminalId: "term-close",
        herdrTabId: "tab-close",
        paneId: "pane-close"
    }
    useWorkspaceStore.setState({
        workspacePath: "/w",
        activeGroupIndex: 1,
        groups: [
            { activePath: null, tabs: [] },
            { activePath: pagePath, tabs: [tab] }
        ]
    })
    useHerdrStore.setState({
        sessions: [{
            name: "default",
            default: true,
            running: true,
            sessionDir: "/tmp/default",
            socketPath: "/tmp/default.sock"
        }],
        selectedSessionName: "default"
    })

    render(<TabBar groupIndex={1} />)
    fireEvent.click(screen.getByLabelText("Close Closable"))
    await waitFor(() => expect(useAppDialogStore.getState().pending?.type).toBe("confirm"))
    useAppDialogStore.getState().respond(true)
    await waitFor(() => expect(finishRuntimeClose).toBeTypeOf("function"))

    act(() => {
        useWorkspaceStore.setState({
            groups: [{ activePath: pagePath, tabs: [tab] }],
            activeGroupIndex: 0
        })
    })
    finishRuntimeClose()

    await waitFor(() => expect(
        useWorkspaceStore.getState().groups.some((group) =>
            group.tabs.some((candidate) => candidate.path === pagePath)
        )
    ).toBe(false))
})

test("closing a stale Herdr page treats the matching runtime tab_not_found as idempotent success", async () => {
    const closeRequests: Array<Record<string, unknown>> = []
    mockIPC((cmd, args) => {
        if (cmd === "herdr_tab_close") {
            closeRequests.push(args as Record<string, unknown>)
            throw new Error("tab_not_found: tab tab-stale not found")
        }
        if (cmd === "log_event") return null
        return undefined
    })
    const pagePath = "yuzora://herdr/default/term-stale"
    useWorkspaceStore.setState({
        workspacePath: "/w",
        activeGroupIndex: 0,
        groups: [{
            activePath: pagePath,
            tabs: [{
                path: pagePath,
                name: "Renamed stale page",
                dirty: false,
                externallyModified: false,
                kind: "herdr-terminal",
                herdrSessionId: "default",
                terminalId: "term-stale",
                herdrTabId: "tab-stale",
                paneId: "pane-stale"
            }]
        }]
    })
    useHerdrStore.setState({
        sessions: [{
            name: "default",
            default: true,
            running: true,
            sessionDir: "/tmp/default",
            socketPath: "/tmp/default.sock"
        }],
        selectedSessionName: "default",
        snapshot: {
            herdrSessionId: "default",
            protocol: 19,
            version: "0.8.0",
            spaces: [],
            agents: [],
            tabs: [],
            terminals: [],
            raw: {}
        },
        connectionState: "ready"
    })

    render(<TabBar groupIndex={0} />)
    fireEvent.click(screen.getByLabelText("Close Renamed stale page"))

    await waitFor(() => expect(useAppDialogStore.getState().pending?.type).toBe("confirm"))
    useAppDialogStore.getState().respond(true)
    await waitFor(() => expect(
        useWorkspaceStore.getState().groups[0].tabs.some((tab) => tab.path === pagePath)
    ).toBe(false))
    expect(closeRequests).toEqual([{
        sessionName: "default",
        tabId: "tab-stale"
    }])
    expect(nativeMessage).not.toHaveBeenCalled()
})

test("failed Herdr tab.close keeps the local page open", async () => {
    mockIPC((cmd) => {
        if (cmd === "herdr_tab_close") {
            throw new Error("tab_not_found: tab another-tab not found")
        }
        if (cmd === "log_event") return null
        return undefined
    })
    const pagePath = "yuzora://herdr/default/term-close"
    useWorkspaceStore.setState({
        workspacePath: "/w",
        activeGroupIndex: 0,
        groups: [{
            activePath: pagePath,
            tabs: [{
                path: pagePath,
                name: "Closable",
                dirty: false,
                externallyModified: false,
                kind: "herdr-terminal",
                herdrSessionId: "default",
                terminalId: "term-close",
                herdrTabId: "tab-close",
                paneId: "pane-close"
            }]
        }]
    })
    useHerdrStore.setState({
        sessions: [{
            name: "default",
            default: true,
            running: true,
            sessionDir: "/tmp/default",
            socketPath: "/tmp/default.sock"
        }],
        selectedSessionName: "default"
    })

    render(<TabBar groupIndex={0} />)
    fireEvent.click(screen.getByLabelText("Close Closable"))

    await waitFor(() => expect(useAppDialogStore.getState().pending?.type).toBe("confirm"))
    useAppDialogStore.getState().respond(true)
    await waitFor(() => expect(useAppDialogStore.getState().pending).toMatchObject({ type: "message" }))
    expect(useWorkspaceStore.getState().groups[0].tabs.some((tab) => tab.path === pagePath)).toBe(true)
})

test("Herdr terminal tab focuses its runtime tab and opens the typed destructive menu", () => {
    const activateTab = vi.fn().mockResolvedValue({ ok: true })
    const pagePath = "yuzora://herdr/default/term-1"
    useWorkspaceStore.setState({
        workspacePath: "/w",
        activeGroupIndex: 0,
        groups: [
            {
                activePath: pagePath,
                tabs: [
                    {
                        path: pagePath,
                        name: "Shell",
                        dirty: false,
                        externallyModified: false,
                        kind: "herdr-terminal",
                        herdrSessionId: "default",
                        terminalId: "term-1",
                        herdrTabId: "tab-1",
                        paneId: "pane-1"
                    }
                ]
            }
        ]
    })
    useHerdrStore.setState({
        sessions: [
            {
                name: "default",
                default: true,
                running: true,
                sessionDir: "/tmp/default",
                socketPath: "/tmp/default.sock"
            }
        ],
        selectedSessionName: "default",
        snapshot: {
            herdrSessionId: "default",
            protocol: 19,
            version: "0.8.0",
            spaces: [],
            agents: [],
            tabs: [
                {
                    id: "tab-1",
                    label: "Shell",
                    order: 1,
                    workspaceId: "ws-1",
                    paneCount: 1,
                    status: "idle",
                    active: true,
                    focused: true,
                    paneId: "pane-1",
                    terminalId: "term-1",
                    sessionName: "default"
                }
            ],
            terminals: [
                {
                    terminalId: "term-1",
                    paneId: "pane-1",
                    tabId: "tab-1",
                    workspaceId: "ws-1"
                }
            ],
            raw: {}
        },
        activateTab
    })

    render(<TabBar groupIndex={0} />)
    fireEvent.click(screen.getByText("Shell"))
    expect(activateTab).toHaveBeenCalledWith(
        expect.objectContaining({ id: "tab-1", terminalId: "term-1" })
    )
    fireEvent.contextMenu(screen.getByText("Shell"))
    expect(useContextMenuStore.getState().request).toMatchObject({
        kind: "herdrTab",
        sessionName: "default",
        tabId: "tab-1",
        workspaceId: "ws-1",
        pagePath
    })
})

test("already-open Herdr tabs switch locally and stale repeated clicks cannot roll them back", async () => {
    type ActivationResult = { ok: true } | { ok: false; cancelled: true }
    const finishActivations: Array<(result: ActivationResult) => void> = []
    const activateTab = vi.fn(
        () =>
            new Promise<ActivationResult>((resolve) => {
                finishActivations.push(resolve)
            })
    )
    const firstPath = "yuzora://herdr/default/term-1"
    const secondPath = "yuzora://herdr/default/term-2"
    useWorkspaceStore.setState({
        workspacePath: "/w",
        activeGroupIndex: 0,
        groups: [
            {
                activePath: firstPath,
                tabs: [
                    {
                        path: firstPath,
                        name: "First",
                        dirty: false,
                        externallyModified: false,
                        kind: "herdr-terminal",
                        herdrSessionId: "default",
                        terminalId: "term-1",
                        herdrTabId: "tab-1",
                        paneId: "pane-1"
                    },
                    {
                        path: secondPath,
                        name: "Second",
                        dirty: false,
                        externallyModified: false,
                        kind: "herdr-terminal",
                        herdrSessionId: "default",
                        terminalId: "term-2",
                        herdrTabId: "tab-2",
                        paneId: "pane-2"
                    }
                ]
            }
        ]
    })
    useHerdrStore.setState({
        sessions: [
            {
                name: "default",
                default: true,
                running: true,
                sessionDir: "/tmp/default",
                socketPath: "/tmp/default.sock"
            }
        ],
        selectedSessionName: "default",
        snapshot: {
            herdrSessionId: "default",
            protocol: 19,
            version: "0.8.0",
            spaces: [],
            agents: [],
            tabs: [
                {
                    id: "tab-1",
                    label: "First",
                    order: 1,
                    workspaceId: "ws-1",
                    paneCount: 1,
                    status: "idle",
                    active: true,
                    focused: true,
                    paneId: "pane-1",
                    terminalId: "term-1",
                    sessionName: "default"
                },
                {
                    id: "tab-2",
                    label: "Second",
                    order: 2,
                    workspaceId: "ws-1",
                    paneCount: 1,
                    status: "idle",
                    active: false,
                    focused: false,
                    paneId: "pane-2",
                    terminalId: "term-2",
                    sessionName: "default"
                }
            ],
            terminals: [],
            raw: {}
        },
        activateTab
    })

    render(<TabBar groupIndex={0} />)
    fireEvent.click(screen.getByText("Second"))
    fireEvent.click(screen.getByText("Second"))

    expect(useWorkspaceStore.getState().groups[0].activePath).toBe(secondPath)
    expect(activateTab).toHaveBeenCalledTimes(2)
    expect(activateTab).toHaveBeenLastCalledWith(
        expect.objectContaining({ id: "tab-2" })
    )

    await act(async () => {
        finishActivations[0]({ ok: false, cancelled: true })
        await Promise.resolve()
    })
    expect(useWorkspaceStore.getState().groups[0].activePath).toBe(secondPath)

    await act(async () => {
        finishActivations[1]({ ok: true })
        await Promise.resolve()
    })
    expect(useWorkspaceStore.getState().groups[0].activePath).toBe(secondPath)
})

test("tab 新增選單在非 ADE 模式也能把 Browser 開到目前 editor group", async () => {
    useUiStore.setState({ mode: "files" })
    useWorkspaceStore.setState({
        workspacePath: "/w",
        activeGroupIndex: 1,
        groups: [
            { activePath: "/w/a.ts", tabs: [{ path: "/w/a.ts", name: "a.ts", dirty: false, externallyModified: false }] },
            { activePath: null, tabs: [] }
        ]
    })

    render(<TabBar groupIndex={1} />)
    fireEvent.pointerDown(screen.getByTestId("tabs-add-menu-1"), {
        button: 0,
        ctrlKey: false
    })
    fireEvent.click(await screen.findByTestId("open-browser-tab-menu-item"))

    expect(useWorkspaceStore.getState().groups[1]).toMatchObject({
        activePath: PREVIEW_TAB_PATH,
        tabs: [expect.objectContaining({ path: PREVIEW_TAB_PATH, kind: "preview" })]
    })
    expect(useWorkspaceStore.getState().groups[0].tabs).not.toContainEqual(
        expect.objectContaining({ path: PREVIEW_TAB_PATH })
    )
})

test("TabBar projects only the selected Space's Herdr pages", () => {
    const firstPath = "yuzora://herdr/default/term-1"
    const secondPath = "yuzora://herdr/default/term-2"
    useWorkspaceStore.setState({
        workspacePath: "/w",
        activeGroupIndex: 0,
        groups: [{
            activePath: secondPath,
            tabs: [
                {
                    path: firstPath,
                    name: "Space One",
                    dirty: false,
                    externallyModified: false,
                    kind: "herdr-terminal",
                    herdrSessionId: "default",
                    terminalId: "term-1",
                    herdrTabId: "tab-1",
                    herdrWorkspaceId: "ws-1"
                },
                {
                    path: secondPath,
                    name: "Space Two",
                    dirty: false,
                    externallyModified: false,
                    kind: "herdr-terminal",
                    herdrSessionId: "default",
                    terminalId: "term-2",
                    herdrTabId: "tab-2",
                    herdrWorkspaceId: "ws-2"
                }
            ]
        }]
    })
    useHerdrStore.setState({
        sessions: [{
            name: "default",
            default: true,
            running: true,
            sessionDir: "/tmp/default",
            socketPath: "/tmp/default.sock"
        }],
        selectedSessionName: "default",
        selectedSpaceId: "ws-1",
        snapshot: {
            herdrSessionId: "default",
            protocol: 19,
            version: "0.8.0",
            spaces: [
                { id: "ws-1", label: "One", order: 1, focused: true },
                { id: "ws-2", label: "Two", order: 2, focused: false }
            ],
            agents: [],
            tabs: [
                { id: "tab-1", label: "Space One", order: 1, workspaceId: "ws-1", paneCount: 1, status: "idle", active: true, focused: true, terminalId: "term-1", sessionName: "default" },
                { id: "tab-2", label: "Space Two", order: 2, workspaceId: "ws-2", paneCount: 1, status: "idle", active: true, focused: false, terminalId: "term-2", sessionName: "default" }
            ],
            terminals: [],
            raw: {}
        }
    })

    render(<TabBar groupIndex={0} />)

    expect(screen.getByText("Space One")).toBeInTheDocument()
    expect(screen.queryByText("Space Two")).not.toBeInTheDocument()

    act(() => {
        useHerdrStore.setState({ selectedSpaceId: "ws-2" })
    })
    expect(screen.queryByText("Space One")).not.toBeInTheDocument()
    expect(screen.getByText("Space Two")).toBeInTheDocument()
})

test("ADE tab menu lists existing Herdr tabs and activates the selected runtime tab", async () => {
    const activateTab = vi.fn().mockResolvedValue({ ok: true })
    useWorkspaceStore.setState({
        workspacePath: "/w",
        activeGroupIndex: 0,
        groups: [{ activePath: null, tabs: [] }]
    })
    useHerdrStore.setState({
        sessions: [
            {
                name: "default",
                default: true,
                running: true,
                sessionDir: "/tmp/default",
                socketPath: "/tmp/default.sock"
            }
        ],
        selectedSessionName: "default",
        selectedSpaceId: "ws-1",
        snapshot: {
            herdrSessionId: "default",
            protocol: 19,
            version: "0.8.0",
            spaces: [{ id: "ws-1", label: "Main", order: 1, focused: true }],
            agents: [],
            tabs: [
                {
                    id: "tab-1",
                    label: "Agent",
                    order: 1,
                    workspaceId: "ws-1",
                    paneCount: 2,
                    status: "idle",
                    active: true,
                    focused: true,
                    paneId: "pane-1",
                    terminalId: "term-1",
                    sessionName: "default"
                }
            ],
            terminals: [],
            focusedWorkspaceId: "ws-1",
            focusedTabId: "tab-1",
            focusedPaneId: "pane-1",
            raw: {}
        },
        canCreateTerminal: () => true,
        canMutateSelectedSession: () => true,
        canFocusSelectedTab: () => true,
        activateTab
    })

    render(<TabBar groupIndex={0} />)
    fireEvent.pointerDown(screen.getByTestId("tabs-add-menu-0"), {
        button: 0,
        ctrlKey: false
    })
    const existing = await screen.findByTestId("herdr-open-tab-tab-1")
    expect(existing).toHaveTextContent("Agent")
    expect(existing).toHaveTextContent("2 panes")
    expect(existing).toHaveTextContent("Focused")

    fireEvent.click(existing)
    await waitFor(() => expect(activateTab).toHaveBeenCalledWith(
        expect.objectContaining({ id: "tab-1", terminalId: "term-1" })
    ))

    act(() => {
        useHerdrStore.setState({ canFocusSelectedTab: () => false })
    })
    fireEvent.pointerDown(screen.getByTestId("tabs-add-menu-0"), {
        button: 0,
        ctrlKey: false
    })
    expect(await screen.findByTestId("herdr-open-tab-tab-1")).toHaveAttribute("data-disabled")
})

test("ADE tab menu creates a persistent Herdr tab and immediately requests its name", async () => {
    const createTerminalInSelectedSpace = vi.fn().mockResolvedValue({
        herdrSessionId: "default",
        terminalId: "term-new",
        paneId: "pane-new",
        tabId: "tab-new",
        title: "New shell"
    })
    useWorkspaceStore.setState({
        workspacePath: "/w",
        activeGroupIndex: 0,
        groups: [{ activePath: null, tabs: [] }]
    })
    useHerdrStore.setState({
        selectedSessionName: "default",
        selectedSpaceId: "ws-1",
        snapshot: {
            herdrSessionId: "default",
            protocol: 19,
            version: "0.8.0",
            spaces: [{ id: "ws-1", label: "Main", order: 1, focused: true }],
            agents: [],
            tabs: [],
            terminals: [],
            raw: {}
        },
        canCreateTerminal: () => true,
        canFocusSelectedTab: () => true,
        createTerminalInSelectedSpace
    })

    render(<TabBar groupIndex={0} />)
    fireEvent.pointerDown(screen.getByTestId("tabs-add-menu-0"), {
        button: 0,
        ctrlKey: false
    })
    fireEvent.click(await screen.findByTestId("herdr-new-tab-menu-item"))

    await waitFor(() => {
        expect(useTextInputDialogStore.getState().pending).toMatchObject({
            initialValue: "New shell"
        })
    })
    useTextInputDialogStore.getState().respond("Build shell")

    await waitFor(() => {
        expect(useWorkspaceStore.getState().groups[0].tabs[0]).toMatchObject({
            name: "Build shell",
            terminalId: "term-new",
            herdrTabId: "tab-new",
            paneId: "pane-new"
        })
    })
    expect(createTerminalInSelectedSpace).toHaveBeenCalledTimes(1)
    expect(herdrTabRename).toHaveBeenCalledWith({
        sessionName: "default",
        tabId: "tab-new",
        label: "Build shell"
    })
})

test("tab path tooltip 移除 extended prefix，但 context target 保留 raw path", () => {
    const rawPath = "\\\\?\\C:\\Work\\專案 空間\\a.ts"
    useWorkspaceStore.getState().setWorkspace("\\\\?\\C:\\Work\\專案 空間")
    useWorkspaceStore.getState().openTab(rawPath)
    render(<TabBar groupIndex={0} />)

    const tabName = screen.getByRole("button", { name: "a.ts" })
    expect(tabName).toHaveAttribute("title", "C:\\Work\\專案 空間\\a.ts")

    fireEvent.contextMenu(tabName)
    expect(useContextMenuStore.getState().request).toMatchObject({
        kind: "tab",
        workspacePath: "\\\\?\\C:\\Work\\專案 空間",
        path: rawPath,
        groupIndex: 0
    })
})

test("ordinary file tabs reorder through HTML5 drag/drop", () => {
    mockIPC((cmd) => (cmd === "log_event" ? null : undefined))
    seedTabs()
    render(<TabBar groupIndex={0} />)
    const source = screen.getByRole("button", { name: "a.ts" })
    const target = screen.getByText("b.ts").closest(".tab")
    expect(source).toHaveAttribute("draggable", "true")
    const dataTransfer = {
        effectAllowed: "none",
        setData: vi.fn(),
        getData: () => "/w/a.ts"
    }
    fireEvent.dragStart(source, { dataTransfer })
    fireEvent.drop(target!, { dataTransfer })
    expect(useWorkspaceStore.getState().groups[0].tabs.map((tab) => tab.path)).toEqual([
        "/w/b.ts",
        "/w/a.ts"
    ])
    expect(useWorkspaceStore.getState().groups[0].activePath).toBe("/w/a.ts")
})

test("ordinary drag permutes projected slots without displacing a hidden-Space page", () => {
    mockIPC((cmd) => (cmd === "log_event" ? null : undefined))
    const hiddenPath = "yuzora://herdr/default/hidden"
    useWorkspaceStore.setState({
        workspacePath: "/w",
        activeGroupIndex: 0,
        groups: [{
            activePath: "/w/a.ts",
            tabs: [
                { path: "/w/a.ts", name: "a.ts", dirty: true, externallyModified: false },
                {
                    path: hiddenPath,
                    name: "Hidden Space",
                    dirty: false,
                    externallyModified: false,
                    kind: "herdr-terminal",
                    herdrSessionId: "default",
                    terminalId: "term-hidden",
                    herdrTabId: "tab-hidden",
                    herdrWorkspaceId: "ws-hidden"
                },
                { path: "/w/b.ts", name: "b.ts", dirty: false, externallyModified: false }
            ]
        }]
    })
    useHerdrStore.setState({
        sessions: [{
            name: "default",
            default: true,
            running: true,
            sessionDir: "/tmp/default",
            socketPath: "/tmp/default.sock"
        }],
        selectedSessionName: "default",
        selectedSpaceId: "ws-visible",
        snapshot: {
            herdrSessionId: "default",
            protocol: 19,
            version: "0.8.0",
            spaces: [
                { id: "ws-visible", label: "Visible", order: 0, focused: true },
                { id: "ws-hidden", label: "Hidden", order: 1, focused: false }
            ],
            agents: [],
            tabs: [],
            terminals: [],
            raw: {}
        }
    })

    render(<TabBar groupIndex={0} />)
    expect(screen.queryByText("Hidden Space")).not.toBeInTheDocument()
    const source = screen.getByRole("button", { name: "a.ts" })
    const target = screen.getByText("b.ts").closest(".tab")
    const dataTransfer = {
        effectAllowed: "none",
        setData: vi.fn(),
        getData: () => "/w/a.ts"
    }
    fireEvent.dragStart(source, { dataTransfer })
    fireEvent.drop(target!, { dataTransfer })

    expect(useWorkspaceStore.getState().groups[0].tabs.map((tab) => tab.path)).toEqual([
        "/w/b.ts",
        hiddenPath,
        "/w/a.ts"
    ])
    expect(useWorkspaceStore.getState().groups[0].tabs[1]).toMatchObject({
        path: hiddenPath,
        herdrWorkspaceId: "ws-hidden"
    })
})

test("Alt+Arrow reorders ordinary projected slots without activation or hidden-Space displacement", () => {
    mockIPC((cmd) => (cmd === "log_event" ? null : undefined))
    const hiddenPath = "yuzora://herdr/default/hidden-keyboard"
    useWorkspaceStore.setState({
        workspacePath: "/w",
        activeGroupIndex: 0,
        groups: [{
            activePath: "/w/a.ts",
            tabs: [
                { path: "/w/a.ts", name: "a.ts", dirty: false, externallyModified: false },
                {
                    path: hiddenPath,
                    name: "Hidden Keyboard Space",
                    dirty: false,
                    externallyModified: false,
                    kind: "herdr-terminal",
                    herdrSessionId: "default",
                    terminalId: "term-hidden-keyboard",
                    herdrTabId: "tab-hidden-keyboard",
                    herdrWorkspaceId: "ws-hidden"
                },
                { path: "/w/b.ts", name: "b.ts", dirty: false, externallyModified: false }
            ]
        }]
    })
    useHerdrStore.setState({
        selectedSessionName: "default",
        selectedSpaceId: "ws-visible",
        snapshot: {
            herdrSessionId: "default",
            protocol: 19,
            version: "0.8.0",
            spaces: [],
            agents: [],
            tabs: [],
            terminals: [],
            raw: {}
        }
    })

    render(<TabBar groupIndex={0} />)
    const first = screen.getByRole("button", { name: "a.ts" })
    const second = screen.getByRole("button", { name: "b.ts" })
    expect(second).toHaveAttribute("aria-keyshortcuts", "Alt+ArrowLeft Alt+ArrowRight")
    expect(fireEvent.keyDown(first, { key: "ArrowLeft", altKey: true })).toBe(true)
    expect(fireEvent.keyDown(second, { key: "ArrowLeft", altKey: true })).toBe(false)

    expect(useWorkspaceStore.getState().groups[0].tabs.map((tab) => tab.path)).toEqual([
        "/w/b.ts",
        hiddenPath,
        "/w/a.ts"
    ])
    expect(useWorkspaceStore.getState().groups[0].activePath).toBe("/w/a.ts")
    expect(useWorkspaceStore.getState().groups[0].tabs).toHaveLength(3)
})

test("Herdr tabs stay undraggable without tab.move and do not local-reorder", () => {
    const firstPath = "yuzora://herdr/default/term-1"
    const secondPath = "yuzora://herdr/default/term-2"
    useWorkspaceStore.setState({
        workspacePath: "/w",
        activeGroupIndex: 0,
        groups: [{
            activePath: firstPath,
            tabs: [
                {
                    path: firstPath,
                    name: "Space One",
                    dirty: false,
                    externallyModified: false,
                    kind: "herdr-terminal",
                    herdrSessionId: "default",
                    terminalId: "term-1",
                    herdrTabId: "tab-1",
                    herdrWorkspaceId: "ws-1"
                },
                {
                    path: secondPath,
                    name: "Space Two",
                    dirty: false,
                    externallyModified: false,
                    kind: "herdr-terminal",
                    herdrSessionId: "default",
                    terminalId: "term-2",
                    herdrTabId: "tab-2",
                    herdrWorkspaceId: "ws-1"
                }
            ]
        }]
    })
    useHerdrStore.setState({
        sessions: [{
            name: "default",
            default: true,
            running: true,
            sessionDir: "/tmp/default",
            socketPath: "/tmp/default.sock"
        }],
        selectedSessionName: "default",
        selectedSpaceId: "ws-1",
        snapshot: {
            herdrSessionId: "default",
            protocol: 19,
            version: "0.8.0",
            spaces: [{ id: "ws-1", label: "One", order: 1, focused: true }],
            agents: [],
            tabs: [
                { id: "tab-1", label: "Space One", order: 1, workspaceId: "ws-1", paneCount: 1, status: "idle", active: true, focused: true, terminalId: "term-1", sessionName: "default" },
                { id: "tab-2", label: "Space Two", order: 2, workspaceId: "ws-1", paneCount: 1, status: "idle", active: false, focused: false, terminalId: "term-2", sessionName: "default" }
            ],
            terminals: [],
            raw: {}
        }
    })
    render(<TabBar groupIndex={0} />)
    const source = screen.getByRole("button", { name: "Space One" })
    expect(source).toHaveAttribute("draggable", "false")
    const dataTransfer = {
        effectAllowed: "none",
        setData: vi.fn(),
        getData: () => firstPath
    }
    fireEvent.dragStart(source, { dataTransfer })
    fireEvent.drop(screen.getByText("Space Two").closest(".tab")!, { dataTransfer })
    expect(herdrTabMove).not.toHaveBeenCalled()
    expect(useWorkspaceStore.getState().groups[0].tabs.map((tab) => tab.path)).toEqual([
        firstPath,
        secondPath
    ])
})

test("legacy Herdr tab without stored Space identity reorders from runtime ownership", async () => {
    mockIPC((cmd) => (cmd === "log_event" ? null : undefined))
    const firstPath = "yuzora://herdr/default/term-1"
    const secondPath = "yuzora://herdr/default/term-2"
    const hiddenPath = "yuzora://herdr/default/hidden"
    const refreshSnapshot = vi.fn().mockResolvedValue(undefined)
    useWorkspaceStore.setState({
        workspacePath: "/w",
        activeGroupIndex: 0,
        groups: [{
            activePath: firstPath,
            tabs: [
                {
                    path: firstPath,
                    name: "One",
                    dirty: false,
                    externallyModified: false,
                    kind: "herdr-terminal",
                    herdrSessionId: "default",
                    terminalId: "term-1",
                    herdrTabId: "tab-1"
                },
                {
                    path: hiddenPath,
                    name: "Hidden",
                    dirty: false,
                    externallyModified: false,
                    kind: "herdr-terminal",
                    herdrSessionId: "default",
                    terminalId: "term-h",
                    herdrTabId: "tab-h",
                    herdrWorkspaceId: "ws-2"
                },
                {
                    path: secondPath,
                    name: "Two",
                    dirty: false,
                    externallyModified: false,
                    kind: "herdr-terminal",
                    herdrSessionId: "default",
                    terminalId: "term-2",
                    herdrTabId: "tab-2",
                    herdrWorkspaceId: "ws-1"
                }
            ]
        }]
    })
    useHerdrStore.setState({
        sessions: [{
            name: "default",
            default: true,
            running: true,
            sessionDir: "/tmp/default",
            socketPath: "/tmp/default.sock"
        }],
        selectedSessionName: "default",
        selectedSpaceId: "ws-1",
        canMoveSelectedTab: () => true,
        canMutateSelectedSession: () => true,
        capabilities: {
            ...initialHerdrState.capabilities,
            binarySource: {
                configured: "global",
                available: true,
                restartRequired: false
            },
            server: { running: true },
            api: {
                snapshot: true,
                ping: true,
                tabCreate: true,
                workspaceFocus: true,
                workspaceCreate: true,
                workspaceRename: true,
                workspaceClose: true,
                tabRename: true,
                tabClose: true,
                tabFocus: true,
                tabMove: true,
                paneFocus: true,
                paneRename: true,
                paneSplit: true,
                paneZoom: true,
                paneSwap: true,
                paneClose: true,
                layoutExport: true,
                layoutSetSplitRatio: true,
                agentGet: true,
                agentRead: true,
                eventsSubscribe: true,
                worktreeList: true,
                methods: ["tab.move"]
            },
            terminal: {
                observe: true,
                control: true,
                takeover: true,
                input: true,
                resize: true,
                scroll: true,
                release: true,
                create: true
            },
            events: { status: "available" }
        },
        refreshSnapshot,
        snapshot: {
            herdrSessionId: "default",
            protocol: 19,
            version: "0.8.0",
            spaces: [
                { id: "ws-1", label: "One", order: 1, focused: true },
                { id: "ws-2", label: "Two", order: 2, focused: false }
            ],
            agents: [],
            tabs: [
                { id: "tab-1", label: "One", order: 1, workspaceId: "ws-1", paneCount: 1, status: "idle", active: true, focused: true, terminalId: "term-1", sessionName: "default" },
                { id: "tab-2", label: "Two", order: 2, workspaceId: "ws-1", paneCount: 1, status: "idle", active: false, focused: false, terminalId: "term-2", sessionName: "default" }
            ],
            terminals: [],
            raw: {}
        }
    })
    render(<TabBar groupIndex={0} />)
    const source = screen.getByRole("button", { name: "One" })
    expect(source).toHaveAttribute("draggable", "true")
    const dataTransfer = {
        effectAllowed: "none",
        setData: vi.fn(),
        getData: () => firstPath
    }
    fireEvent.dragStart(source, { dataTransfer })
    fireEvent.drop(screen.getByText("Two").closest(".tab")!, { dataTransfer })
    await waitFor(() => expect(herdrTabMove).toHaveBeenCalledWith({
        sessionName: "default",
        tabId: "tab-1",
        insertIndex: 1
    }))
    expect(refreshSnapshot).toHaveBeenCalledWith("default")
    expect(useWorkspaceStore.getState().groups[0].tabs.map((tab) => tab.path)).toEqual([
        firstPath,
        hiddenPath,
        secondPath
    ])
})

test("Alt+Arrow uses schema-gated tab.move for Herdr tabs", async () => {
    mockIPC((cmd) => (cmd === "log_event" ? null : undefined))
    const firstPath = "yuzora://herdr/default/keyboard-1"
    const secondPath = "yuzora://herdr/default/keyboard-2"
    const refreshSnapshot = vi.fn().mockResolvedValue(undefined)
    useWorkspaceStore.setState({
        workspacePath: "/w",
        activeGroupIndex: 0,
        groups: [{
            activePath: firstPath,
            tabs: [
                {
                    path: firstPath,
                    name: "Keyboard One",
                    dirty: false,
                    externallyModified: false,
                    kind: "herdr-terminal",
                    herdrSessionId: "default",
                    terminalId: "keyboard-term-1",
                    herdrTabId: "keyboard-tab-1"
                },
                {
                    path: secondPath,
                    name: "Keyboard Two",
                    dirty: false,
                    externallyModified: false,
                    kind: "herdr-terminal",
                    herdrSessionId: "default",
                    terminalId: "keyboard-term-2",
                    herdrTabId: "keyboard-tab-2"
                }
            ]
        }]
    })
    useHerdrStore.setState({
        sessions: [{
            name: "default",
            default: true,
            running: true,
            sessionDir: "/tmp/default",
            socketPath: "/tmp/default.sock"
        }],
        selectedSessionName: "default",
        selectedSpaceId: "ws-1",
        canMoveSelectedTab: () => false,
        refreshSnapshot,
        snapshot: {
            herdrSessionId: "default",
            protocol: 19,
            version: "0.8.0",
            spaces: [{ id: "ws-1", label: "One", order: 1, focused: true }],
            agents: [],
            tabs: [
                { id: "keyboard-tab-1", label: "Keyboard One", order: 1, workspaceId: "ws-1", paneCount: 1, status: "idle", active: true, focused: true, terminalId: "keyboard-term-1", sessionName: "default" },
                { id: "keyboard-tab-2", label: "Keyboard Two", order: 2, workspaceId: "ws-1", paneCount: 1, status: "idle", active: false, focused: false, terminalId: "keyboard-term-2", sessionName: "default" }
            ],
            terminals: [],
            raw: {}
        }
    })

    render(<TabBar groupIndex={0} />)
    const source = screen.getByRole("button", { name: "Keyboard One" })
    expect(source).toHaveAttribute("aria-keyshortcuts", "Alt+ArrowLeft Alt+ArrowRight")
    expect(source).toHaveAttribute("draggable", "false")
    expect(fireEvent.keyDown(source, { key: "ArrowRight", altKey: true })).toBe(true)
    expect(herdrTabMove).not.toHaveBeenCalled()

    act(() => {
        useHerdrStore.setState({ canMoveSelectedTab: () => true })
    })
    expect(source).toHaveAttribute("draggable", "true")
    expect(fireEvent.keyDown(source, { key: "ArrowLeft", altKey: true })).toBe(true)
    expect(fireEvent.keyDown(source, { key: "ArrowRight", altKey: true })).toBe(false)

    await waitFor(() => expect(herdrTabMove).toHaveBeenCalledWith({
        sessionName: "default",
        tabId: "keyboard-tab-1",
        insertIndex: 1
    }))
    expect(refreshSnapshot).toHaveBeenCalledWith("default")
    expect(useWorkspaceStore.getState().groups[0].tabs.map((tab) => tab.path)).toEqual([
        firstPath,
        secondPath
    ])
    expect(useWorkspaceStore.getState().groups[0].activePath).toBe(firstPath)
})

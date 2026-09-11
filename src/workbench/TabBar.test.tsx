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

test("clicking a tab host badge selects the same page as its name", () => {
    mockIPC((cmd) => (cmd === "log_event" ? null : undefined))
    seedTabs()
    render(<TabBar groupIndex={0} />)
    const badge = screen.getByText("b.ts").closest(".tab")!.querySelector('[data-slot="badge"]')!
    expect(badge).toBeTruthy()
    fireEvent.click(badge)
    expect(useWorkspaceStore.getState().groups[0].activePath).toBe("/w/b.ts")
})

test("vertical wheel input cannot move the horizontal tab strip", () => {
    seedTabs()
    const { container } = render(<TabBar groupIndex={0} />)
    const viewport = container.querySelector('[data-slot="scroll-area-viewport"]')!
    const vertical = new WheelEvent("wheel", { bubbles: true, cancelable: true, deltaY: 80 })
    viewport.dispatchEvent(vertical)
    expect(vertical.defaultPrevented).toBe(true)
    for (const init of [{ deltaX: 80 }, { deltaY: 80, shiftKey: true }, { deltaY: 80, ctrlKey: true }]) {
        const intentional = new WheelEvent("wheel", { bubbles: true, cancelable: true, ...init })
        viewport.dispatchEvent(intentional)
        expect(intentional.defaultPrevented).toBe(false)
    }
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

test("Markdown opens in its own document tab without an extra preview button", () => {
    mockIPC((cmd) => (cmd === "open_file" ? { kind: "full", content: "", size: 0 } : undefined))
    seedMdTab()
    render(<TabBar groupIndex={0} />)
    expect(screen.queryByLabelText("Toggle preview r.md")).toBeNull()
    expect(screen.queryByLabelText("Toggle preview a.ts")).toBeNull()
})

test("Alt+P pins a tab first without changing its active identity", () => {
    seedTabs()
    render(<TabBar groupIndex={0} />)
    fireEvent.keyDown(screen.getByRole("button", { name: "b.ts" }), { key: "p", altKey: true })
    const group = useWorkspaceStore.getState().groups[0]
    expect(group.tabs[0]).toMatchObject({ path: "/w/b.ts", pinned: true, dirty: true })
    expect(group.activePath).toBe("/w/a.ts")
    fireEvent.keyDown(screen.getByRole("button", { name: "b.ts" }), { key: "p", altKey: true })
    expect(useWorkspaceStore.getState().groups[0].tabs.find((tab) => tab.path === "/w/b.ts")?.pinned).toBe(false)
})

test("TabBar 只管理 toggle，不再 mount Markdown preview", () => {
    seedMdTab()
    useWorkspaceStore.getState().toggleMarkdownPreview("/w/r.md", 0)
    render(<TabBar groupIndex={0} />)
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

test("SVG retains its preview toggle while Markdown uses document mode", () => {
    seedMixedPreviewTabs()
    render(<TabBar groupIndex={0} />)
    expect(screen.getByLabelText("Toggle preview logo.svg")).toHaveAttribute("aria-pressed", "true")
    expect(screen.queryByLabelText("Toggle preview r.md")).toBeNull()
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

test.each([false, true])("the second terminal close button closes its exact runtime tab before removing the page (already gone: %s)", async (alreadyGone) => {
    const commands: Array<{ cmd: string; args: unknown }> = []
    let finishClose!: () => void
    mockIPC((cmd, args) => {
        commands.push({ cmd, args })
        if (cmd === "herdr_tab_close") return new Promise<void>((resolve, reject) => {
            finishClose = () => alreadyGone ? reject("tab_not_found: tab tab-close not found") : resolve()
        })
        return null
    })
    const release = vi.fn().mockResolvedValue(undefined)
    const refresh = vi.fn().mockResolvedValue(true)
    const path = "yuzora://herdr/default/term-close"
    const first = { path: "yuzora://herdr/default/term-first", name: "First", kind: "herdr-terminal" as const, herdrSessionId: "default", terminalId: "term-first", herdrTabId: "tab-first", dirty: false, externallyModified: false }
    useWorkspaceStore.setState({ workspacePath: "/w", activeGroupIndex: 0, groups: [{ activePath: path, tabs: [first, { path, name: "Shell", kind: "herdr-terminal", herdrSessionId: "default", terminalId: "term-close", herdrTabId: "tab-close", dirty: false, externallyModified: false }] }] })
    useHerdrStore.setState({ selectedSpaceId: null, selectedSessionName: "default", sessions: [{ name: "default", default: true, running: true, sessionDir: "/tmp/default", socketPath: "/tmp/default.sock" }], capabilities: { server: { running: true }, api: { tabClose: true, methods: ["tab.close"] } } as NonNullable<typeof initialHerdrState.capabilities>, releaseAttachmentsForPage: release, refreshSnapshot: refresh })
    render(<TabBar groupIndex={0} />)
    fireEvent.click(screen.getByLabelText("Close Shell"))
    await waitFor(() => expect(commands).toContainEqual({ cmd: "herdr_tab_close", args: { sessionName: "default", tabId: "tab-close" } }))
    expect(useWorkspaceStore.getState().groups[0].tabs).toHaveLength(2)
    expect(release).not.toHaveBeenCalled()
    fireEvent.click(screen.getByLabelText("Close Shell"))
    expect(commands.filter((item) => item.cmd === "herdr_tab_close")).toHaveLength(1)
    await act(async () => { finishClose() })
    await waitFor(() => expect(useWorkspaceStore.getState().groups[0].tabs).toEqual([first]))
    expect(release).toHaveBeenCalledWith(path)
    expect(refresh).toHaveBeenCalledWith("default")
    expect(useAppDialogStore.getState().pending).toBeNull()
})

test("a failed terminal close preserves its page and attachment", async () => {
    mockIPC((cmd) => { if (cmd === "herdr_tab_close") throw new Error("host disconnected"); return null })
    const release = vi.fn().mockResolvedValue(undefined)
    const path = "yuzora://herdr/default/term-close"
    useWorkspaceStore.setState({ activeGroupIndex: 0, groups: [{ activePath: path, tabs: [{ path, name: "Shell", kind: "herdr-terminal", herdrSessionId: "default", terminalId: "term-close", herdrTabId: "tab-close", dirty: false, externallyModified: false }] }] })
    useHerdrStore.setState({ selectedSpaceId: null, selectedSessionName: "default", sessions: [{ name: "default", default: true, running: true, sessionDir: "/tmp/default", socketPath: "/tmp/default.sock" }], capabilities: { server: { running: true }, api: { tabClose: true, methods: ["tab.close"] } } as NonNullable<typeof initialHerdrState.capabilities>, releaseAttachmentsForPage: release })
    render(<TabBar groupIndex={0} />)
    fireEvent.click(screen.getByLabelText("Close Shell"))
    await waitFor(() => expect(useAppDialogStore.getState().pending).not.toBeNull())
    expect(useWorkspaceStore.getState().groups[0].tabs).toHaveLength(1)
    expect(release).not.toHaveBeenCalled()
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
    expect(second).toHaveAttribute("aria-keyshortcuts", "Alt+ArrowLeft Alt+ArrowRight Alt+P")
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
    expect(source).toHaveAttribute("aria-keyshortcuts", "Alt+ArrowLeft Alt+ArrowRight Alt+P")
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

test("reveals an externally activated clipped tab without scrolling on focus or metadata updates", () => {
    seedTabs()
    const { container } = render(<TabBar groupIndex={0} />)
    const viewport = container.querySelector('[data-slot="scroll-area-viewport"]') as HTMLElement
    Object.defineProperty(viewport, "clientWidth", { configurable: true, value: 650 })
    Object.defineProperty(viewport, "scrollWidth", { configurable: true, value: 902 })
    vi.spyOn(viewport, "getBoundingClientRect").mockReturnValue({ left: 312, right: 962, top: 0, bottom: 44, width: 650, height: 44 } as DOMRect)
    const first = screen.getByRole("button", { name: "a.ts" })
    const second = screen.getByRole("button", { name: "b.ts" })
    vi.spyOn(first.closest(".tab")!, "getBoundingClientRect").mockImplementation(() => ({ left: 312 - viewport.scrollLeft, right: 512 - viewport.scrollLeft, width: 200 } as DOMRect))
    vi.spyOn(second.closest(".tab")!, "getBoundingClientRect").mockImplementation(() => ({ left: 1027 - viewport.scrollLeft, right: 1149 - viewport.scrollLeft, width: 122 } as DOMRect))
    fireEvent.focus(second)
    fireEvent.mouseDown(second, { button: 0 })
    expect(viewport.scrollLeft).toBe(0)
    act(() => useWorkspaceStore.getState().setActiveTab(0, "/w/b.ts"))
    expect(viewport.scrollLeft).toBe(187)
    // A user may pan the strip while reading; passive store updates must not snap it back.
    viewport.scrollLeft = 0
    act(() => useWorkspaceStore.getState().markDirty("/w/b.ts", false))
    expect(viewport.scrollLeft).toBe(0)
    viewport.scrollLeft = 187
    act(() => useWorkspaceStore.getState().setActiveTab(0, "/w/a.ts"))
    expect(viewport.scrollLeft).toBe(0)
})

test("opening a new offscreen tab reveals it in the strip while preserving ancestor scroll", () => {
    seedTabs()
    const { container } = render(<div data-testid="outer-scroll"><TabBar groupIndex={0} /></div>)
    const outer = screen.getByTestId("outer-scroll")
    outer.scrollTop = 73
    const viewport = container.querySelector('[data-slot="scroll-area-viewport"]') as HTMLElement
    Object.defineProperty(viewport, "clientWidth", { configurable: true, value: 300 })
    Object.defineProperty(viewport, "scrollWidth", { configurable: true, value: 700 })
    const rect = vi.spyOn(HTMLElement.prototype, "getBoundingClientRect").mockImplementation(function (this: HTMLElement) {
        if (this === viewport) return { left: 0, right: 300, width: 300 } as DOMRect
        if (this.classList.contains("tab") && this.classList.contains("active")) return { left: 550, right: 700, width: 150 } as DOMRect
        return { left: 0, right: 0, width: 0 } as DOMRect
    })
    try {
        act(() => useWorkspaceStore.getState().openTab("/w/agents.sql"))
        expect(viewport.scrollLeft).toBe(400)
        expect(outer.scrollTop).toBe(73)
    } finally { rect.mockRestore() }
})

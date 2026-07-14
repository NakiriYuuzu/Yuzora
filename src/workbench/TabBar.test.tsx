import { expect, test, afterEach, beforeEach, vi } from "vitest"
import { render, screen, fireEvent, waitFor } from "@testing-library/react"
import { mockIPC, clearMocks } from "@tauri-apps/api/mocks"

// The dirty-close flow now routes through confirmDialogStore's imperative gate
// (replacing the Tauri native confirm()). Mock the store so tests drive the
// decision, mock saveDirtyTab so the "save" branch doesn't touch the editor/ipc,
// and mock @tauri-apps/plugin-dialog so we can assert native confirm is gone.
// vi.hoisted because these are referenced inside the hoisted vi.mock factories
// (plugin-dialog loads very early via contextMenuStore, before plain consts init).
const { requestUnsavedDecision, nativeConfirm } = vi.hoisted(() => ({
    requestUnsavedDecision: vi.fn(),
    nativeConfirm: vi.fn()
}))
vi.mock("../state/confirmDialogStore", () => ({
    useConfirmDialogStore: { getState: () => ({ requestUnsavedDecision }) }
}))
vi.mock("@tauri-apps/plugin-dialog", () => ({ confirm: nativeConfirm, message: vi.fn() }))
vi.mock("../editor/saveDocument", () => ({ saveDirtyTab: vi.fn() }))

import { TabBar } from "./TabBar"
import { PREVIEW_TAB_PATH, useWorkspaceStore } from "../state/workspaceStore"
import { useContextMenuStore } from "../state/contextMenuStore"
import { useSvgPreviewStore } from "../state/svgPreviewStore"
import { useUiStore, uiInitialState } from "../state/uiStore"
import { useMarkdownPreviewStore } from "./MarkdownPreview"
import { saveDirtyTab } from "../editor/saveDocument"

beforeEach(() => {
    requestUnsavedDecision.mockReset()
    nativeConfirm.mockReset()
    vi.mocked(saveDirtyTab).mockReset().mockResolvedValue(undefined)
})

afterEach(() => {
    clearMocks()
    useContextMenuStore.setState({ request: null, x: 0, y: 0, availabilityRevision: 0 })
    useUiStore.setState(uiInitialState)
    useMarkdownPreviewStore.setState({ openPaths: {} })
    useSvgPreviewStore.getState().reset()
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
    expect(useMarkdownPreviewStore.getState().isOpen("/w/r.md")).toBe(false)
    fireEvent.click(screen.getByLabelText("Toggle preview r.md"))
    expect(useMarkdownPreviewStore.getState().isOpen("/w/r.md")).toBe(true)
    fireEvent.click(screen.getByLabelText("Toggle preview r.md"))
    expect(useMarkdownPreviewStore.getState().isOpen("/w/r.md")).toBe(false)
})

test("TabBar 只管理 toggle，不再 mount Markdown preview", () => {
    seedMdTab()
    useMarkdownPreviewStore.setState({ openPaths: { "/w/r.md": true } })
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
    useMarkdownPreviewStore.setState({ openPaths: { "/w/r.md": true } })
    render(<TabBar groupIndex={0} />)
    fireEvent.click(screen.getByLabelText("Close r.md"))
    await waitFor(() =>
        expect(useMarkdownPreviewStore.getState().isOpen("/w/r.md")).toBe(false)
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
    expect(useMarkdownPreviewStore.getState().isOpen("/w/r.md")).toBe(true)
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

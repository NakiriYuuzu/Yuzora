import { act, cleanup, fireEvent, render, screen } from "@testing-library/react"
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"

import { uiInitialState, useUiStore } from "../state/uiStore"
import { useWorkspaceStore } from "../state/workspaceStore"

vi.mock("../editor/EditorPane", () => ({
    EditorPane: ({ path }: { path: string }) => <div data-testid="editor-pane-mock">{path}</div>
}))
vi.mock("../editor/documentRegistry", () => ({
    documentGeneration: vi.fn(() => 0),
    getDocument: vi.fn(async () => ({
        result: { kind: "full", content: "<svg xmlns='http://www.w3.org/2000/svg'/>", size: 42 }
    }))
}))
vi.mock("../editor/viewRegistry", () => ({ getView: vi.fn(() => undefined) }))
vi.mock("@/app/panels/PreviewPanel", () => ({
    PreviewPanel: () => <div data-testid="preview-panel-mock" />
}))

import { SvgSplitView, isSvgPath, useSvgPreviewStore } from "./SvgSplitView"
import { EditorArea } from "./EditorArea"
import { getDocument } from "../editor/documentRegistry"

const createdUrls = vi.hoisted(() => ({ count: 0, revoked: [] as string[] }))

beforeEach(() => {
    vi.stubGlobal("URL", {
        ...URL,
        createObjectURL: vi.fn(() => `blob:mock-${++createdUrls.count}`),
        revokeObjectURL: vi.fn((url: string) => createdUrls.revoked.push(url))
    })
    useUiStore.setState({ ...uiInitialState, mode: "files" })
    useWorkspaceStore.setState({
        workspacePath: "/ws",
        groups: [
            {
                tabs: [
                    {
                        path: "/ws/logo.svg",
                        name: "logo.svg",
                        dirty: false,
                        externallyModified: false,
                        kind: "file" as const
                    }
                ],
                activePath: "/ws/logo.svg"
            }
        ],
        activeGroupIndex: 0,
        pendingReveal: null
    })
    useSvgPreviewStore.getState().reset()
})

afterEach(() => {
    cleanup()
    vi.unstubAllGlobals()
    vi.clearAllMocks()
    createdUrls.count = 0
    createdUrls.revoked.length = 0
})

describe("isSvgPath", () => {
    it("只接受 .svg（含大小寫），排除其他格式", () => {
        expect(isSvgPath("a.svg")).toBe(true)
        expect(isSvgPath("b.SVG")).toBe(true)
        expect(isSvgPath("c.png")).toBe(false)
        expect(isSvgPath("svg")).toBe(false)
        expect(isSvgPath("a.svg.md")).toBe(false)
    })
})

describe("SvgSplitView", () => {
    it("uses a Windows basename for preview alt while loading the raw path", async () => {
        const rawPath = String.raw`\\?\C:\Work\中文 workspace\logo.svg`

        render(<SvgSplitView path={rawPath} groupIndex={0} />)

        expect(await screen.findByTestId("svg-preview-img")).toHaveAttribute("alt", "logo.svg")
        expect(getDocument).toHaveBeenCalledWith(rawPath)
    })

    it("開檔預設開啟預覽（與 Markdown 的預設關閉相反）", async () => {
        render(<SvgSplitView path="/ws/logo.svg" groupIndex={0} />)

        expect(screen.getByTestId("editor-pane-mock")).toBeInTheDocument()
        expect(screen.getByTestId("svg-preview")).toBeInTheDocument()
        expect(await screen.findByTestId("svg-preview-img")).toBeInTheDocument()
    })

    it("明確關閉會被記住；forget 後回到預設開啟", () => {
        const { unmount } = render(<SvgSplitView path="/ws/logo.svg" groupIndex={0} />)

        act(() => useSvgPreviewStore.getState().toggle("/ws/logo.svg"))
        expect(screen.queryByTestId("svg-preview")).not.toBeInTheDocument()

        unmount()
        render(<SvgSplitView path="/ws/logo.svg" groupIndex={0} />)
        expect(screen.queryByTestId("svg-preview")).not.toBeInTheDocument()

        act(() => useSvgPreviewStore.getState().forget("/ws/logo.svg"))
        expect(screen.getByTestId("svg-preview")).toBeInTheDocument()
    })

    it("workspace 切換清空明確關閉狀態（W8 慣例）", () => {
        act(() => useSvgPreviewStore.getState().toggle("/ws/logo.svg"))
        expect(useSvgPreviewStore.getState().isOpen("/ws/logo.svg")).toBe(false)

        act(() => useWorkspaceStore.getState().setWorkspace("/other"))

        expect(useSvgPreviewStore.getState().isOpen("/ws/logo.svg")).toBe(true)
    })

    it("blob URL 於 unmount 時 revoke", async () => {
        const { unmount } = render(<SvgSplitView path="/ws/logo.svg" groupIndex={0} />)
        const img = (await screen.findByTestId("svg-preview-img")) as HTMLImageElement
        const firstUrl = img.src

        unmount()

        expect(createdUrls.revoked).toContain(firstUrl.replace(window.location.origin + "/", ""))
    })

    it("content 替換（path 切換）revoke 舊 blob URL——create/revoke 配對、使用中 URL 不 revoke", async () => {
        const { rerender } = render(<SvgSplitView path="/ws/logo.svg" groupIndex={0} />)
        await screen.findByTestId("svg-preview-img")
        expect(createdUrls.count).toBe(1)

        rerender(<SvgSplitView path="/ws/other.svg" groupIndex={0} />)

        await vi.waitFor(() => expect(createdUrls.count).toBe(2))
        expect(createdUrls.revoked).toContain("blob:mock-1")
        expect(createdUrls.revoked).not.toContain("blob:mock-2")
    })

    it("SVG 渲染失敗顯示 placeholder，不影響左側編輯", async () => {
        render(<SvgSplitView path="/ws/logo.svg" groupIndex={0} />)
        const img = await screen.findByTestId("svg-preview-img")

        fireEvent.error(img)

        expect(screen.getByText("Cannot render this SVG")).toBeInTheDocument()
        expect(screen.getByTestId("editor-pane-mock")).toBeInTheDocument()
    })
})

describe("EditorArea 分支", () => {
    it("svg path 走 SvgSplitView（編輯器＋預覽並排）", async () => {
        render(<EditorArea />)

        expect(screen.getByTestId("svg-split-view")).toBeInTheDocument()
        expect(screen.getByTestId("editor-pane-mock")).toBeInTheDocument()
        expect(screen.getByTestId("svg-preview")).toBeInTheDocument()
    })
})

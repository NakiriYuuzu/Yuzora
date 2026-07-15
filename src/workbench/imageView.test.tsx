import { cleanup, fireEvent, render, screen } from "@testing-library/react"
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"

import { useWorkspaceStore } from "../state/workspaceStore"

vi.mock("@tauri-apps/api/core", () => ({
    convertFileSrc: vi.fn((path: string) => `asset://localhost/${encodeURIComponent(path)}`)
}))
vi.mock("@/lib/ipc", () => ({
    openFile: vi.fn(async () => ({ kind: "binary", size: 245760 }))
}))
vi.mock("../editor/EditorPane", () => ({
    EditorPane: ({ path }: { path: string }) => <div data-testid="editor-pane-mock">{path}</div>
}))
vi.mock("../editor/documentRegistry", () => ({
    documentGeneration: vi.fn(() => 0),
    getDocument: vi.fn(async () => ({ result: { kind: "full", content: "", size: 0 } }))
}))
vi.mock("../editor/viewRegistry", () => ({ getView: vi.fn(() => undefined) }))
vi.mock("@/app/panels/PreviewPanel", () => ({
    PreviewPanel: () => <div data-testid="preview-panel-mock" />
}))

import { ImageView, isImagePath } from "./ImageView"
import { EditorArea } from "./EditorArea"

function loadImage(width: number, height: number) {
    const img = screen.getByTestId("image-view-img") as HTMLImageElement
    Object.defineProperty(img, "naturalWidth", { value: width, configurable: true })
    Object.defineProperty(img, "naturalHeight", { value: height, configurable: true })
    fireEvent.load(img)
    return img
}

afterEach(() => {
    cleanup()
    vi.clearAllMocks()
})

describe("isImagePath", () => {
    it("接受常見 binary 圖片副檔名（含大小寫與多點檔名）", () => {
        for (const name of [
            "a.png",
            "b.JPG",
            "c.jpeg",
            "d.gif",
            "e.webp",
            "f.BMP",
            "g.ico",
            "h.avif",
            "shot.v2.final.PNG"
        ]) {
            expect(isImagePath(name), name).toBe(true)
        }
    })

    it("排除 SVG（走 SvgSplitView）、文字檔與無副檔名", () => {
        for (const name of ["logo.svg", "a.ts", "README", "png", "image.png.md"]) {
            expect(isImagePath(name), name).toBe(false)
        }
    })
})

describe("ImageView", () => {
    it("以 asset URL 載圖；onLoad 後狀態列顯示尺寸、檔案大小與縮放比", async () => {
        render(<ImageView path="/ws/logo.png" />)

        const img = screen.getByTestId("image-view-img") as HTMLImageElement
        expect(img.src).toBe(`asset://localhost/${encodeURIComponent("/ws/logo.png")}`)

        loadImage(1024, 768)

        const meta = await screen.findByTestId("image-view-meta")
        expect(meta.textContent).toContain("1024×768")
        expect(meta.textContent).toContain("240.0 KB")
        expect(meta.textContent).toMatch(/\d+%/)
    })

    it("1:1 設為 100%；＋／−依倍率縮放並夾在 10%–800%", () => {
        render(<ImageView path="/ws/logo.png" />)
        loadImage(100, 100)

        fireEvent.click(screen.getByRole("button", { name: "Actual size (1:1)" }))
        expect(screen.getByTestId("image-view-meta").textContent).toContain("100%")

        fireEvent.click(screen.getByRole("button", { name: "Zoom in" }))
        expect(screen.getByTestId("image-view-meta").textContent).toContain("125%")

        for (let i = 0; i < 30; i++) {
            fireEvent.click(screen.getByRole("button", { name: "Zoom in" }))
        }
        expect(screen.getByTestId("image-view-meta").textContent).toContain("800%")

        for (let i = 0; i < 60; i++) {
            fireEvent.click(screen.getByRole("button", { name: "Zoom out" }))
        }
        expect(screen.getByTestId("image-view-meta").textContent).toContain("10%")
    })

    it("Cmd/Ctrl＋滾輪縮放（preventDefault 阻止捲動）；一般滾輪不攔截、不改縮放", () => {
        render(<ImageView path="/ws/logo.png" />)
        loadImage(100, 100)
        fireEvent.click(screen.getByRole("button", { name: "Actual size (1:1)" }))

        const surface = screen.getByTestId("image-view-img").parentElement as HTMLElement
        // 縮放走 ref 掛的 non-passive listener：fireEvent 回傳 false 代表
        // preventDefault 已生效（React 的 root wheel listener 是 passive，擋不住）。
        expect(fireEvent.wheel(surface, { deltaY: -120, ctrlKey: true })).toBe(false)
        expect(screen.getByTestId("image-view-meta").textContent).toContain("125%")

        expect(fireEvent.wheel(surface, { deltaY: -120 })).toBe(true)
        expect(screen.getByTestId("image-view-meta").textContent).toContain("125%")
    })

    it("載入失敗顯示安全路徑，不洩漏 Windows extended prefix", () => {
        const rawPath = String.raw`\\?\C:\Work\中文 workspace\broken.png`
        const displayPath = String.raw`C:\Work\中文 workspace\broken.png`
        render(<ImageView path={rawPath} />)

        fireEvent.error(screen.getByTestId("image-view-img"))

        expect(screen.getByText("Could not load this image")).toBeInTheDocument()
        expect(screen.getByText(new RegExp(displayPath.replace(/[.*+?^${}()|[\]\\]/g, "\\$&")))).toBeInTheDocument()
        expect(screen.queryByText(new RegExp(rawPath.replace(/[.*+?^${}()|[\]\\]/g, "\\$&")))).not.toBeInTheDocument()
    })
})

describe("EditorArea 分支", () => {
    beforeEach(() => {
        useWorkspaceStore.setState({
            workspacePath: "/ws",
            groups: [
                {
                    tabs: [
                        {
                            path: "/ws/logo.png",
                            name: "logo.png",
                            dirty: false,
                            externallyModified: false,
                            kind: "file" as const
                        }
                    ],
                    activePath: "/ws/logo.png"
                }
            ],
            activeGroupIndex: 0,
            pendingReveal: null
        })
    })

    it("image path 走 ImageView（不建立文字編輯器）", () => {
        render(<EditorArea />)

        expect(screen.getByTestId("image-view")).toBeInTheDocument()
        expect(screen.queryByTestId("editor-pane-mock")).not.toBeInTheDocument()
    })
})

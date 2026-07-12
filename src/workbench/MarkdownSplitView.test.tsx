import { act, cleanup, fireEvent, render, screen } from "@testing-library/react"
import { afterEach, beforeEach, expect, test, vi } from "vitest"

import { uiInitialState, useUiStore } from "../state/uiStore"
import { PREVIEW_TAB_PATH, useWorkspaceStore } from "../state/workspaceStore"
import {
    WORKBENCH_LAYOUT_STORAGE_KEY,
    useWorkbenchLayoutStore,
    workbenchLayoutInitialState
} from "../state/workbenchLayoutStore"
import { useMarkdownPreviewStore } from "./MarkdownPreview"
import { getView } from "../editor/viewRegistry"

const editorIdentity = vi.hoisted(() => ({ mounts: 0, unmounts: 0 }))

vi.mock("../editor/EditorPane", async () => {
    const React = await import("react")
    return {
        EditorPane: ({ path }: { path: string }) => {
            const [identity] = React.useState(() => ++editorIdentity.mounts)
            React.useEffect(() => () => {
                editorIdentity.unmounts++
            }, [])
            return <div data-testid="editor-pane-mock" data-identity={identity}>{path}</div>
        }
    }
})

vi.mock("../editor/documentRegistry", () => ({
    documentGeneration: vi.fn(() => 0),
    dropDocument: vi.fn(),
    getDocument: vi.fn(async () => ({ result: { kind: "full", content: "# Preview", size: 9 } }))
}))
vi.mock("../editor/viewRegistry", () => ({ getView: vi.fn(() => undefined) }))
vi.mock("@tauri-apps/plugin-opener", () => ({ openUrl: vi.fn(async () => undefined) }))
vi.mock("@/app/panels/PreviewPanel", () => ({
    PreviewPanel: () => <div data-testid="preview-panel-mock" />
}))

import { MarkdownSplitView } from "./MarkdownSplitView"
import { EditorArea } from "./EditorArea"

interface ObserverHarness {
    trigger: (width: number, height: number) => void
}

const observers: ObserverHarness[] = []

function installLocalStorage() {
    const values = new Map<string, string>()
    Object.defineProperty(globalThis, "localStorage", {
        configurable: true,
        writable: true,
        value: {
            getItem: (key: string) => values.get(key) ?? null,
            setItem: (key: string, value: string) => void values.set(key, String(value)),
            removeItem: (key: string) => void values.delete(key),
            clear: () => values.clear(),
            key: (index: number) => [...values.keys()][index] ?? null,
            get length() {
                return values.size
            }
        }
    })
}

class ResizeObserverHarness {
    observe = vi.fn()
    unobserve = vi.fn()
    disconnect = vi.fn()

    constructor(callback: ResizeObserverCallback) {
        observers.push({
            trigger: (width, height) => callback(
                [{ contentRect: { width, height } } as unknown as ResizeObserverEntry],
                this as unknown as ResizeObserver
            )
        })
    }
}

function seed(previewOpen = true) {
    useUiStore.setState(uiInitialState)
    useWorkspaceStore.setState({
        workspacePath: "/w",
        activeGroupIndex: 0,
        groups: [
            {
                activePath: "/w/readme.md",
                tabs: [
                    {
                        path: "/w/readme.md",
                        name: "readme.md",
                        dirty: false,
                        externallyModified: false
                    }
                ]
            }
        ]
    })
    useMarkdownPreviewStore.setState({
        openPaths: previewOpen ? { "/w/readme.md": true } : {}
    })
    useWorkbenchLayoutStore.setState({ ...workbenchLayoutInitialState })
}

function renderSplit(previewOpen = true) {
    seed(previewOpen)
    const result = render(<MarkdownSplitView path="/w/readme.md" groupIndex={0} />)
    const split = screen.getByTestId("markdown-split-view")
    Object.defineProperty(split, "getBoundingClientRect", {
        configurable: true,
        value: () => ({
            x: 0,
            y: 0,
            top: 0,
            left: 0,
            right: 806,
            bottom: 600,
            width: 806,
            height: 600,
            toJSON: () => ({})
        })
    })
    return result
}

function resize(width: number, height: number) {
    act(() => observers.at(-1)?.trigger(width, height))
}

beforeEach(() => {
    observers.length = 0
    editorIdentity.mounts = 0
    editorIdentity.unmounts = 0
    installLocalStorage()
    localStorage.removeItem(WORKBENCH_LAYOUT_STORAGE_KEY)
    globalThis.ResizeObserver = ResizeObserverHarness as unknown as typeof ResizeObserver
    vi.mocked(getView).mockReturnValue(undefined)
})

afterEach(() => {
    cleanup()
    useMarkdownPreviewStore.setState({ openPaths: {} })
})

test("preview toggle 與 focus/mode gate 不 remount EditorPane", () => {
    renderSplit(false)
    resize(806, 600)
    const editor = screen.getByTestId("editor-pane-mock")
    const identity = editor.dataset.identity
    expect(screen.queryByRole("complementary", { name: "Markdown preview" })).toBeNull()

    act(() => useMarkdownPreviewStore.getState().toggle("/w/readme.md"))
    expect(screen.getByRole("complementary", { name: "Markdown preview" })).toBeTruthy()
    expect(screen.getByTestId("editor-pane-mock").dataset.identity).toBe(identity)

    act(() => useWorkspaceStore.setState({ activeGroupIndex: 1 }))
    expect(screen.queryByRole("complementary", { name: "Markdown preview" })).toBeNull()
    expect(screen.getByTestId("editor-pane-mock").dataset.identity).toBe(identity)

    act(() => {
        useWorkspaceStore.setState({ activeGroupIndex: 0 })
        useUiStore.setState({ mode: "git" })
    })
    expect(screen.queryByRole("complementary", { name: "Markdown preview" })).toBeNull()
    expect(screen.getByTestId("editor-pane-mock").dataset.identity).toBe(identity)
    expect(editorIdentity.mounts).toBe(1)
    expect(editorIdentity.unmounts).toBe(0)
})

test("兩個 Markdown groups 同時為 open 時只顯示 focused group 的 preview", () => {
    seed()
    useWorkspaceStore.setState({
        activeGroupIndex: 0,
        groups: [
            {
                activePath: "/w/readme.md",
                tabs: [{ path: "/w/readme.md", name: "readme.md", dirty: false, externallyModified: false }]
            },
            {
                activePath: "/w/notes.md",
                tabs: [{ path: "/w/notes.md", name: "notes.md", dirty: false, externallyModified: false }]
            }
        ]
    })
    useMarkdownPreviewStore.setState({
        openPaths: { "/w/readme.md": true, "/w/notes.md": true }
    })
    render(
        <>
            <MarkdownSplitView path="/w/readme.md" groupIndex={0} />
            <MarkdownSplitView path="/w/notes.md" groupIndex={1} />
        </>
    )
    expect(screen.getAllByRole("complementary", { name: "Markdown preview" })).toHaveLength(1)

    act(() => useWorkspaceStore.setState({ activeGroupIndex: 1 }))
    expect(screen.getAllByRole("complementary", { name: "Markdown preview" })).toHaveLength(1)
    expect(screen.getAllByTestId("editor-pane-mock")).toHaveLength(2)
})

test("inactive group 不 mount preview，也不嘗試建立 scroll coordinator", () => {
    seed()
    useWorkspaceStore.setState({ activeGroupIndex: 0 })
    vi.mocked(getView).mockClear()

    render(<MarkdownSplitView path="/w/readme.md" groupIndex={1} />)

    expect(screen.queryByRole("complementary", { name: "Markdown preview" })).toBeNull()
    expect(getView).not.toHaveBeenCalled()
})

test("EditorArea 對 Markdown 固定掛 split wrapper，其他 document/PreviewPanel 分支不變", () => {
    seed(false)
    render(<EditorArea />)
    expect(screen.getByTestId("markdown-split-view")).toBeTruthy()
    const markdownIdentity = screen.getByTestId("editor-pane-mock").dataset.identity

    act(() => useMarkdownPreviewStore.getState().toggle("/w/readme.md"))
    expect(screen.getByTestId("editor-pane-mock").dataset.identity).toBe(markdownIdentity)

    act(() => useWorkspaceStore.setState({
        groups: [
            {
                activePath: "/w/main.ts",
                tabs: [
                    {
                        path: "/w/main.ts",
                        name: "main.ts",
                        dirty: false,
                        externallyModified: false
                    }
                ]
            }
        ]
    }))
    expect(screen.queryByTestId("markdown-split-view")).toBeNull()
    expect(screen.getByTestId("editor-pane-mock")).toHaveTextContent("/w/main.ts")

    act(() => useWorkspaceStore.setState({
        groups: [
            {
                activePath: PREVIEW_TAB_PATH,
                tabs: [
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
    }))
    expect(screen.getByTestId("preview-panel-mock")).toBeTruthy()
    expect(screen.queryByTestId("markdown-split-view")).toBeNull()
})

test("ResizeObserver 以 640px 切換 orientation，且不改 persisted ratio", () => {
    renderSplit()
    useWorkbenchLayoutStore.setState({ markdownEditorRatio: 0.6 })

    resize(640, 500)
    expect(screen.getByTestId("markdown-split-view")).toHaveAttribute("data-orientation", "row")
    expect(screen.getByRole("separator")).toHaveAttribute("aria-orientation", "vertical")

    resize(639, 500)
    expect(screen.getByTestId("markdown-split-view")).toHaveAttribute("data-orientation", "column")
    expect(screen.getByRole("separator")).toHaveAttribute("aria-orientation", "horizontal")
    expect(useWorkbenchLayoutStore.getState().markdownEditorRatio).toBe(0.6)
})

test("dynamic 160px min clamp 與 cramped 50/50 不污染 stored ratio", () => {
    renderSplit()
    useWorkbenchLayoutStore.setState({ markdownEditorRatio: 0.9 })

    resize(806, 600)
    expect(Number(screen.getByTestId("markdown-editor-surface").style.flexGrow)).toBeCloseTo(0.8)
    expect(useWorkbenchLayoutStore.getState().markdownEditorRatio).toBe(0.9)

    resize(639, 300)
    expect(Number(screen.getByTestId("markdown-editor-surface").style.flexGrow)).toBe(0.5)
    fireEvent.keyDown(screen.getByRole("separator"), { key: "ArrowDown" })
    expect(useWorkbenchLayoutStore.getState().markdownEditorRatio).toBe(0.9)
})

test("stored ratio 超出當前 bounds 時，outward pointer no-op 不 commit，inward drag 仍 commit", () => {
    renderSplit()
    useWorkbenchLayoutStore.setState({ markdownEditorRatio: 0.9 })
    resize(806, 600)
    const divider = screen.getByRole("separator")

    expect(divider).toHaveAttribute("aria-valuenow", "80")
    fireEvent.pointerDown(divider, { button: 0, pointerId: 11, clientX: 643, clientY: 100 })
    fireEvent.pointerMove(divider, { pointerId: 11, clientX: 803, clientY: 100 })
    fireEvent.pointerUp(divider, { pointerId: 11, clientX: 803, clientY: 100 })

    expect(useWorkbenchLayoutStore.getState().markdownEditorRatio).toBe(0.9)
    expect(localStorage.getItem(WORKBENCH_LAYOUT_STORAGE_KEY)).toBeNull()

    resize(1606, 600)
    expect(Number(screen.getByTestId("markdown-editor-surface").style.flexGrow)).toBeCloseTo(0.9)

    resize(806, 600)
    fireEvent.pointerDown(divider, { button: 0, pointerId: 12, clientX: 643, clientY: 100 })
    fireEvent.pointerMove(divider, { pointerId: 12, clientX: 563, clientY: 100 })
    fireEvent.pointerUp(divider, { pointerId: 12, clientX: 563, clientY: 100 })

    expect(useWorkbenchLayoutStore.getState().markdownEditorRatio).toBeCloseTo(0.7)
    expect(JSON.parse(localStorage.getItem(WORKBENCH_LAYOUT_STORAGE_KEY) ?? "{}")).toMatchObject({
        markdownEditorRatio: 0.7
    })
})

test("stored ratio 超出當前 bounds 時，outward ArrowRight no-op 不 write，inward key 仍 commit", () => {
    renderSplit()
    useWorkbenchLayoutStore.setState({ markdownEditorRatio: 0.9 })
    resize(806, 600)
    const divider = screen.getByRole("separator")

    fireEvent.keyDown(divider, { key: "ArrowRight" })
    expect(useWorkbenchLayoutStore.getState().markdownEditorRatio).toBe(0.9)
    expect(localStorage.getItem(WORKBENCH_LAYOUT_STORAGE_KEY)).toBeNull()

    resize(1606, 600)
    expect(Number(screen.getByTestId("markdown-editor-surface").style.flexGrow)).toBeCloseTo(0.9)

    resize(806, 600)
    fireEvent.keyDown(divider, { key: "ArrowLeft" })
    expect(useWorkbenchLayoutStore.getState().markdownEditorRatio).toBeCloseTo(0.78)
    expect(JSON.parse(localStorage.getItem(WORKBENCH_LAYOUT_STORAGE_KEY) ?? "{}")).toMatchObject({
        markdownEditorRatio: 0.78
    })
})

test("axis 恰為 326px 時 160px panes 已無可調空間", () => {
    renderSplit()
    useWorkbenchLayoutStore.setState({ markdownEditorRatio: 0.6 })
    resize(639, 326)
    const divider = screen.getByRole("separator")

    expect(divider).toHaveAttribute("aria-valuemin", "50")
    expect(divider).toHaveAttribute("aria-valuemax", "50")
    expect(divider).toHaveAttribute("aria-valuenow", "50")
    fireEvent.keyDown(divider, { key: "ArrowDown" })
    expect(useWorkbenchLayoutStore.getState().markdownEditorRatio).toBe(0.6)
    expect(localStorage.getItem(WORKBENCH_LAYOUT_STORAGE_KEY)).toBeNull()
})

test("pointer move 只更新 transient ratio，release 才 commit 並隱藏 indicator", () => {
    renderSplit()
    resize(806, 600)
    const divider = screen.getByRole("separator")

    fireEvent.pointerDown(divider, { button: 0, pointerId: 7, clientX: 403, clientY: 100 })
    expect(screen.getByText("Editor 50% · Preview 50%")).toHaveAttribute("aria-hidden", "true")

    fireEvent.pointerMove(divider, { pointerId: 7, clientX: 563, clientY: 100 })
    expect(screen.getByText("Editor 70% · Preview 30%")).toBeTruthy()
    expect(useWorkbenchLayoutStore.getState().markdownEditorRatio).toBe(0.5)
    expect(localStorage.getItem(WORKBENCH_LAYOUT_STORAGE_KEY)).toBeNull()
    expect(divider).toHaveAttribute("aria-valuenow", "70")

    fireEvent.pointerUp(divider, { pointerId: 7, clientX: 563, clientY: 100 })
    expect(useWorkbenchLayoutStore.getState().markdownEditorRatio).toBeCloseTo(0.7)
    expect(JSON.parse(localStorage.getItem(WORKBENCH_LAYOUT_STORAGE_KEY) ?? "{}")).toMatchObject({
        markdownEditorRatio: 0.7
    })
    expect(screen.queryByText("Editor 70% · Preview 30%")).toBeNull()
})

test("pointercancel 與 lost capture 都清理並 commit 最後 transient ratio", () => {
    renderSplit()
    resize(806, 600)
    const divider = screen.getByRole("separator")

    fireEvent.pointerDown(divider, { button: 0, pointerId: 1, clientX: 403 })
    fireEvent.pointerMove(divider, { pointerId: 1, clientX: 483 })
    fireEvent.pointerCancel(divider, { pointerId: 1 })
    expect(useWorkbenchLayoutStore.getState().markdownEditorRatio).toBeCloseTo(0.6)
    expect(screen.queryByText(/Editor .* Preview/)).toBeNull()

    fireEvent.pointerDown(divider, { button: 0, pointerId: 2, clientX: 483 })
    fireEvent.pointerMove(divider, { pointerId: 2, clientX: 323 })
    fireEvent.lostPointerCapture(divider, { pointerId: 2 })
    expect(useWorkbenchLayoutStore.getState().markdownEditorRatio).toBeCloseTo(0.4)
    expect(screen.queryByText(/Editor .* Preview/)).toBeNull()
})

test("keyboard 依 axis 使用 2% 與 Shift 10% 並直接 commit", () => {
    renderSplit()
    resize(806, 806)
    const divider = screen.getByRole("separator")

    fireEvent.keyDown(divider, { key: "ArrowRight" })
    expect(useWorkbenchLayoutStore.getState().markdownEditorRatio).toBeCloseTo(0.52)
    fireEvent.keyDown(divider, { key: "ArrowRight", shiftKey: true })
    expect(useWorkbenchLayoutStore.getState().markdownEditorRatio).toBeCloseTo(0.62)
    fireEvent.keyDown(divider, { key: "ArrowUp" })
    expect(useWorkbenchLayoutStore.getState().markdownEditorRatio).toBeCloseTo(0.62)

    resize(639, 806)
    fireEvent.keyDown(divider, { key: "ArrowDown" })
    expect(useWorkbenchLayoutStore.getState().markdownEditorRatio).toBeCloseTo(0.64)
    fireEvent.keyDown(divider, { key: "ArrowUp", shiftKey: true })
    expect(useWorkbenchLayoutStore.getState().markdownEditorRatio).toBeCloseTo(0.54)
    expect(divider).toHaveAttribute("aria-valuetext", "Editor 54% · Preview 46%")
})

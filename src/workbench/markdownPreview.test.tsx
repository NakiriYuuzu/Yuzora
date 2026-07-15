import { afterEach, expect, test, vi } from "vitest"
import { act, cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react"

import type { OpenFileResult } from "../lib/types"
import { getView } from "../editor/viewRegistry"
import { getDocument } from "../editor/documentRegistry"
import { openUrl } from "@tauri-apps/plugin-opener"
import { useWorkspaceStore } from "../state/workspaceStore"

// Mutable stand-in for the editor buffer the preview reads. Each test seeds it
// before rendering, mirroring the EditorPane.test.tsx documentRegistry mock.
let mockResult: OpenFileResult = { kind: "full", content: "", size: 0, lineEnding: "lf" }
vi.mock("../editor/documentRegistry", () => ({
    getDocument: vi.fn(async () => ({ result: mockResult }))
}))
vi.mock("../editor/viewRegistry", () => ({
    getView: vi.fn(() => undefined)
}))
vi.mock("@tauri-apps/plugin-opener", () => ({
    openUrl: vi.fn(async () => undefined)
}))

const {
    MarkdownPreview,
    renderMarkdown,
    isMarkdownPath,
    useMarkdownPreviewStore,
    __renderMarkdownCallCount
} = await import("./MarkdownPreview")

afterEach(() => {
    cleanup()
    vi.clearAllMocks()
    vi.unstubAllGlobals()
    // clearAllMocks keeps implementations; a test that made getView return a
    // live view must not leak that into the next test's initial buffer read.
    vi.mocked(getView).mockReturnValue(undefined)
    vi.useRealTimers()
    useMarkdownPreviewStore.setState({ openPaths: {} })
})

// A minimal live-view stand-in whose doc content is controllable per tick.
function fakeView(read: () => string) {
    return { state: { doc: { toString: read } } } as unknown as ReturnType<typeof getView>
}

function syncView(initialContent: string) {
    const scrollDOM = document.createElement("div")
    let documentTop = 0
    let content = initialContent
    const selection = { anchor: 7 }

    function doc() {
        const lines = content.split(/\r\n?|\n/)
        return {
            lines: lines.length,
            line: (number: number) => ({ from: (number - 1) * 10 }),
            lineAt: (position: number) => ({ number: Math.floor(position / 10) + 1 }),
            toString: () => content
        }
    }

    const state = { doc: doc(), selection }
    const view = {
        state,
        scrollDOM,
        scaleY: 1,
        get documentTop() {
            return documentTop
        },
        lineBlockAtHeight: (height: number) => ({ from: Math.floor(height / 20) * 10 }),
        lineBlockAt: (position: number) => ({ top: (position / 10) * 20 })
    } as unknown as NonNullable<ReturnType<typeof getView>>

    return {
        view,
        selection,
        setViewportLine(line: number) {
            documentTop = -(line - 1) * 20
            scrollDOM.scrollTop = (line - 1) * 20
        },
        setContent(next: string) {
            content = next
            state.doc = doc()
        }
    }
}

function installSyntheticPreviewGeometry() {
    const originalRect = HTMLElement.prototype.getBoundingClientRect
    const originalScrollHeight = Object.getOwnPropertyDescriptor(HTMLElement.prototype, "scrollHeight")
    const originalClientHeight = Object.getOwnPropertyDescriptor(HTMLElement.prototype, "clientHeight")
    let markerScale = 100

    HTMLElement.prototype.getBoundingClientRect = function () {
        const line = Number(this.getAttribute?.("data-yz-source-line"))
        const scrollTop = this.closest?.(".markdown-preview-body")?.scrollTop ?? 0
        const top = Number.isInteger(line) && line > 0 ? line * markerScale - scrollTop : 0
        return { top, bottom: top, left: 0, right: 0, width: 0, height: 0 } as DOMRect
    }
    Object.defineProperty(HTMLElement.prototype, "scrollHeight", {
        configurable: true,
        get() {
            return this.classList?.contains("markdown-preview-body") ? 1000 : 0
        }
    })
    Object.defineProperty(HTMLElement.prototype, "clientHeight", {
        configurable: true,
        get() {
            return this.classList?.contains("markdown-preview-body") ? 100 : 0
        }
    })

    return {
        setMarkerScale(next: number) {
            markerScale = next
        },
        restore() {
            HTMLElement.prototype.getBoundingClientRect = originalRect
            if (originalScrollHeight) {
                Object.defineProperty(HTMLElement.prototype, "scrollHeight", originalScrollHeight)
            } else {
                delete (HTMLElement.prototype as unknown as Record<string, unknown>).scrollHeight
            }
            if (originalClientHeight) {
                Object.defineProperty(HTMLElement.prototype, "clientHeight", originalClientHeight)
            } else {
                delete (HTMLElement.prototype as unknown as Record<string, unknown>).clientHeight
            }
        }
    }
}

function installAnimationFrameQueue() {
    const callbacks = new Map<number, FrameRequestCallback>()
    let next = 1
    vi.stubGlobal("requestAnimationFrame", (callback: FrameRequestCallback) => {
        const handle = next++
        callbacks.set(handle, callback)
        return handle
    })
    vi.stubGlobal("cancelAnimationFrame", (handle: number) => void callbacks.delete(handle))
    return () => {
        const entry = callbacks.entries().next().value as [number, FrameRequestCallback] | undefined
        if (!entry) return
        callbacks.delete(entry[0])
        act(() => entry[1](0))
    }
}

function installPendingFontSet() {
    const originalFonts = Object.getOwnPropertyDescriptor(document, "fonts")
    const loadingDoneListeners = new Set<EventListenerOrEventListenerObject>()
    let resolveReady!: () => void
    const ready = new Promise<void>((resolve) => {
        resolveReady = resolve
    })
    Object.defineProperty(document, "fonts", {
        configurable: true,
        value: {
            ready,
            addEventListener(type: string, listener: EventListenerOrEventListenerObject) {
                if (type === "loadingdone") loadingDoneListeners.add(listener)
            },
            removeEventListener(type: string, listener: EventListenerOrEventListenerObject) {
                if (type === "loadingdone") loadingDoneListeners.delete(listener)
            }
        }
    })

    return {
        ready,
        resolveReady,
        dispatchLoadingDone() {
            const event = new Event("loadingdone")
            for (const listener of [...loadingDoneListeners]) {
                if (typeof listener === "function") listener(event)
                else listener.handleEvent(event)
            }
        },
        listenerCount: () => loadingDoneListeners.size,
        restore() {
            if (originalFonts) Object.defineProperty(document, "fonts", originalFonts)
            else Reflect.deleteProperty(document, "fonts")
        }
    }
}

// --- renderMarkdown（純函式：markdown-it 渲染＋DOMPurify sanitize） ---

test("renderMarkdown 產出 heading／list／code block 結構", () => {
    const html = renderMarkdown("# 標題\n\n- 一\n- 二\n\n```js\nconst x = 1\n```")
    expect(html).toContain("<h1")
    expect(html).toContain("<ul")
    expect(html).toContain("<li")
    expect(html).toContain("<pre>")
    expect(html).toContain("<code")
})

test("renderMarkdown sanitize 移除 <script>（XSS 硬需求）", () => {
    const html = renderMarkdown("# hi\n\n<script>alert(1)</script>")
    expect(html).not.toContain("<script")
    expect(html).not.toContain("alert(1)")
})

test("renderMarkdown sanitize 移除 onerror 等事件屬性", () => {
    const html = renderMarkdown('<img src=x onerror="alert(1)">')
    expect(html).not.toContain("onerror")
})

test("renderMarkdown 清除 javascript: 協定與 target 屬性（W10）", () => {
    const html = renderMarkdown('<a href="javascript:alert(1)" target="_blank">x</a>')
    expect(html).not.toContain("javascript:")
    expect(html).not.toContain("target")
})

test("renderMarkdown 移除 form／input／button／dialog 等可導航元素（R2-7、R9-3）", () => {
    const html = renderMarkdown(
        '<form action="https://evil.test"><input name="x"><button>去</button></form><dialog open>hi</dialog>'
    )
    expect(html).not.toContain("<form")
    expect(html).not.toContain("<input")
    expect(html).not.toContain("<button")
    expect(html).not.toContain("<dialog")
})

test("renderMarkdown 移除 <style>（防全域 CSS 注入藏匿 app）（R9-1）", () => {
    // style 必須跟在其他內容後——DOMPurify 對單獨開頭的 style 本來就會丟。
    const html = renderMarkdown("<p>x</p><style>#root{display:none}</style>")
    expect(html).not.toContain("<style")
    expect(html).not.toContain("display:none")
})

test("renderMarkdown 移除 style 屬性（防 inline CSS overlay／beacon）（R10-2）", () => {
    const html = renderMarkdown(
        '<a href="https://evil" style="position:fixed;inset:0;z-index:2147483647;opacity:0">x</a>'
    )
    expect(html).not.toContain('style="')
    expect(html).not.toContain("position:fixed")
    // 合法內容不受影響。
    const legit = renderMarkdown("# 標題\n\n```js\nx\n```")
    expect(legit).toContain("<h1")
    expect(legit).toContain("<code")
})

test("renderMarkdown 移除 class 屬性（防 Tailwind utility 重建 overlay）（R11-1）", () => {
    const html = renderMarkdown('<div class="fixed inset-0 z-50 opacity-0">x</div>')
    expect(html).not.toContain("class=")
    // 合法語法（heading／list／code fence／table）仍正確渲染——樣式靠標籤選擇器。
    const legit = renderMarkdown(
        "# H\n\n- a\n- b\n\n```js\nx\n```\n\n| a | b |\n|---|---|\n| 1 | 2 |"
    )
    expect(legit).toContain("<h1")
    expect(legit).toContain("<li")
    expect(legit).toContain("<code")
    expect(legit).toContain("<table")
})

test("renderMarkdown 移除 image map（map／area／usemap）（R3-1）", () => {
    const html = renderMarkdown(
        '<img src=x usemap="#m"><map name="m"><area href="https://evil.test" shape="rect" coords="0,0,9,9"></map>'
    )
    expect(html).not.toContain("<map")
    expect(html).not.toContain("<area")
    expect(html).not.toContain("usemap")
})

test("renderMarkdown 以 parse→annotate→renderer 標記代表性 block 起始行", () => {
    const html = renderMarkdown([
        "# heading",
        "",
        "paragraph",
        "",
        "- list",
        "",
        "> quote",
        "",
        "```ts",
        "code",
        "```",
        "",
        "    indented",
        "",
        "| a | b |",
        "|---|---|",
        "| 1 | 2 |",
        "",
        "---",
        "",
        "<section>raw</section>"
    ].join("\n"))
    const root = document.createElement("div")
    root.innerHTML = html

    expect(root.querySelector("h1")?.getAttribute("data-yz-source-line")).toBe("1")
    expect(root.querySelector("p")?.getAttribute("data-yz-source-line")).toBe("3")
    expect(root.querySelector("ul")?.getAttribute("data-yz-source-line")).toBe("5")
    expect(root.querySelector("blockquote")?.getAttribute("data-yz-source-line")).toBe("7")
    expect(root.querySelector("pre code")?.getAttribute("data-yz-source-line")).toBe("9")
    expect(root.querySelectorAll("pre")[1]?.getAttribute("data-yz-source-line")).toBe("13")
    expect(root.querySelector("table")?.getAttribute("data-yz-source-line")).toBe("15")
    expect(root.querySelector("hr")?.getAttribute("data-yz-source-line")).toBe("19")
    expect(root.querySelector("section")?.previousElementSibling).toHaveAttribute(
        "data-yz-source-line",
        "21"
    )
    expect(root.querySelectorAll('[data-yz-source-anchor="block"]').length).toBeGreaterThan(8)
})

test("raw HTML 無法偽造 trusted source marker，sanitized renderer marker仍保留", () => {
    const html = renderMarkdown(
        '<div data-yz-source-line="999" data-yz-source-anchor="block">literal data-yz-source-line="7"</div>'
    )
    const root = document.createElement("div")
    root.innerHTML = html
    const raw = root.querySelector("div")
    expect(raw).not.toHaveAttribute("data-yz-source-line")
    expect(raw).not.toHaveAttribute("data-yz-source-anchor")
    expect(raw).toHaveTextContent('literal data-yz-source-line="7"')
    expect(raw?.previousElementSibling).toHaveAttribute("data-yz-source-line", "1")
    expect(raw?.previousElementSibling).toHaveAttribute("data-yz-source-anchor", "block")
})

test("raw HTML 的 slash 分隔無法繞過 trusted source marker stripping", () => {
    const html = renderMarkdown(
        '<div id="spoof"/data-yz-source-line="5"/data-yz-source-anchor="block">spoof</div>'
    )
    const root = document.createElement("div")
    root.innerHTML = html
    const raw = root.querySelector("#spoof")

    expect(raw).not.toHaveAttribute("data-yz-source-line")
    expect(raw).not.toHaveAttribute("data-yz-source-anchor")
    expect(raw?.previousElementSibling).toHaveAttribute("data-yz-source-line", "1")
    expect(raw?.previousElementSibling).toHaveAttribute("data-yz-source-anchor", "block")
})

// --- isMarkdownPath ---

test("isMarkdownPath 認 .md 與 .markdown、拒其他", () => {
    expect(isMarkdownPath("readme.md")).toBe(true)
    expect(isMarkdownPath("notes.markdown")).toBe(true)
    expect(isMarkdownPath("a.ts")).toBe(false)
    expect(isMarkdownPath("plain")).toBe(false)
})

// --- MarkdownPreview 元件 ---

test("full grade 檔渲染出對應 HTML", async () => {
    mockResult = { kind: "full", content: "# Title\n\n- one\n- two", size: 20, lineEnding: "lf" }
    render(<MarkdownPreview path="/w/r.md" />)
    await waitFor(() => expect(screen.getByRole("heading", { level: 1 })).toBeTruthy())
    expect(screen.getByText("one")).toBeTruthy()
})

test("synthetic offsets 證明 editor／preview scroll events 雙向接線（非 pixel proof）", async () => {
    const geometry = installSyntheticPreviewGeometry()
    const flushFrame = installAnimationFrameQueue()
    const editor = syncView("# one\n\nparagraph\n\n## end")
    vi.mocked(getView).mockReturnValue(editor.view)
    mockResult = { kind: "full", content: "# one\n\nparagraph\n\n## end", size: 31, lineEnding: "lf" }

    try {
        render(<MarkdownPreview path="/w/sync.md" />)
        const preview = await screen.findByTestId("markdown-preview-body")

        editor.setViewportLine(3)
        fireEvent.scroll(editor.view.scrollDOM)
        flushFrame()
        expect(preview.scrollTop).toBe(300)

        preview.scrollTop = 600
        fireEvent.scroll(preview)
        flushFrame()
        expect(editor.view.scrollDOM.scrollTop).toBe(60)
        expect(editor.view.state.selection).toBe(editor.selection)
    } finally {
        geometry.restore()
    }
})

test("400ms content rerender rebuilds live anchors and preserves preview driver source line", async () => {
    vi.useFakeTimers({ shouldAdvanceTime: true })
    const geometry = installSyntheticPreviewGeometry()
    const flushFrame = installAnimationFrameQueue()
    const initial = "# one\n\nparagraph\n\n## end"
    const editor = syncView(initial)
    const subscribeEditorScroll = vi.spyOn(editor.view.scrollDOM, "addEventListener")
    vi.mocked(getView).mockReturnValue(editor.view)
    mockResult = { kind: "full", content: initial, size: initial.length, lineEnding: "lf" }

    try {
        render(<MarkdownPreview path="/w/live-sync.md" />)
        const preview = await screen.findByTestId("markdown-preview-body")
        await waitFor(() => expect(subscribeEditorScroll).toHaveBeenCalledWith(
            "scroll",
            expect.any(Function),
            { passive: true }
        ))
        preview.scrollTop = 300
        fireEvent.scroll(preview)
        flushFrame()

        geometry.setMarkerScale(150)
        editor.setContent("# one\n\nchanged\n\n## next\n\nlast")
        await act(async () => {
            await vi.advanceTimersByTimeAsync(450)
        })

        expect(screen.getByText("changed")).toBeTruthy()
        expect(preview.scrollTop).toBe(450)
        expect(preview.scrollTop).not.toBe(0)
    } finally {
        geometry.restore()
    }
})

test("late EditorView attach succeeds, and unmount removes scroll sync without ghost writes", async () => {
    vi.useFakeTimers({ shouldAdvanceTime: true })
    const geometry = installSyntheticPreviewGeometry()
    const flushFrame = installAnimationFrameQueue()
    const source = "# one\n\nparagraph\n\n## end"
    mockResult = { kind: "full", content: source, size: source.length, lineEnding: "lf" }
    vi.mocked(getView).mockReturnValue(undefined)

    try {
        const rendered = render(<MarkdownPreview path="/w/late.md" />)
        const preview = await screen.findByTestId("markdown-preview-body")
        const editor = syncView(source)
        vi.mocked(getView).mockReturnValue(editor.view)

        await act(async () => {
            await vi.advanceTimersByTimeAsync(450)
        })
        editor.setViewportLine(3)
        fireEvent.scroll(editor.view.scrollDOM)
        flushFrame()
        expect(preview.scrollTop).toBe(300)

        rendered.unmount()
        preview.scrollTop = 0
        fireEvent.scroll(editor.view.scrollDOM)
        flushFrame()
        expect(preview.scrollTop).toBe(0)
    } finally {
        geometry.restore()
    }
})

test("preview ResizeObserver and image load rebuild live offsets at the current source line", async () => {
    const geometry = installSyntheticPreviewGeometry()
    const flushFrame = installAnimationFrameQueue()
    const resizeCallbacks: ResizeObserverCallback[] = []
    vi.stubGlobal("ResizeObserver", class {
        constructor(callback: ResizeObserverCallback) {
            resizeCallbacks.push(callback)
        }
        observe() {}
        unobserve() {}
        disconnect() {}
    })
    const source = "# one\n\n![diagram](image.png)\n\n## end"
    const editor = syncView(source)
    vi.mocked(getView).mockReturnValue(editor.view)
    mockResult = { kind: "full", content: source, size: source.length, lineEnding: "lf" }

    try {
        render(<MarkdownPreview path="/w/reflow.md" />)
        const preview = await screen.findByTestId("markdown-preview-body")
        const image = screen.getByRole("img", { name: "diagram" })
        preview.scrollTop = 300
        fireEvent.scroll(preview)
        flushFrame()

        geometry.setMarkerScale(150)
        act(() => resizeCallbacks.at(-1)?.([], {} as ResizeObserver))
        expect(preview.scrollTop).toBe(450)

        geometry.setMarkerScale(200)
        fireEvent.load(image)
        expect(preview.scrollTop).toBe(600)
    } finally {
        geometry.restore()
    }
})

test("document fonts ready and loadingdone rebuild live offsets at the current source line", async () => {
    const geometry = installSyntheticPreviewGeometry()
    const flushFrame = installAnimationFrameQueue()
    const fonts = installPendingFontSet()
    vi.stubGlobal("ResizeObserver", class {
        observe() {}
        unobserve() {}
        disconnect() {}
    })
    const source = "# one\n\nparagraph\n\n## end"
    const editor = syncView(source)
    vi.mocked(getView).mockReturnValue(editor.view)
    mockResult = { kind: "full", content: source, size: source.length, lineEnding: "lf" }

    try {
        render(<MarkdownPreview path="/w/font-reflow.md" />)
        const preview = await screen.findByTestId("markdown-preview-body")
        preview.scrollTop = 300
        fireEvent.scroll(preview)
        flushFrame()

        geometry.setMarkerScale(150)
        act(() => fonts.dispatchLoadingDone())
        expect(preview.scrollTop).toBe(450)

        geometry.setMarkerScale(200)
        await act(async () => {
            fonts.resolveReady()
            await fonts.ready
        })
        expect(preview.scrollTop).toBe(600)
    } finally {
        fonts.restore()
        geometry.restore()
    }
})

test("font reflow subscription cleanup removes loadingdone and ignores stale ready callback", async () => {
    const fonts = installPendingFontSet()
    vi.stubGlobal("ResizeObserver", class {
        observe() {}
        unobserve() {}
        disconnect() {}
    })
    const source = "# one\n\nparagraph"
    const editor = syncView(source)
    vi.mocked(getView).mockReturnValue(editor.view)
    mockResult = { kind: "full", content: source, size: source.length, lineEnding: "lf" }

    try {
        const rendered = render(<MarkdownPreview path="/w/font-cleanup.md" />)
        const preview = await screen.findByTestId("markdown-preview-body")
        const queryAnchors = vi.spyOn(preview, "querySelectorAll")
        await waitFor(() => expect(fonts.listenerCount()).toBe(1))

        rendered.unmount()
        queryAnchors.mockClear()
        expect(fonts.listenerCount()).toBe(0)
        fonts.dispatchLoadingDone()
        await act(async () => {
            fonts.resolveReady()
            await fonts.ready
        })
        expect(queryAnchors).not.toHaveBeenCalled()
    } finally {
        fonts.restore()
    }
})

test("limited grade 顯示降級提示、不渲染內文", async () => {
    mockResult = { kind: "limited", content: "# 很大的檔案", size: 99_999_999, lineEnding: "lf" }
    render(<MarkdownPreview path="/w/big.md" />)
    await waitFor(() => expect(screen.getByTestId("markdown-preview-downgrade")).toBeTruthy())
    expect(screen.queryByRole("heading", { level: 1 })).toBeNull()
})

test("tooLarge grade 顯示降級提示", async () => {
    mockResult = { kind: "tooLarge", size: 999_999_999 }
    render(<MarkdownPreview path="/w/huge.md" />)
    await waitFor(() => expect(screen.getByTestId("markdown-preview-downgrade")).toBeTruthy())
})

test("空內容顯示空狀態提示", async () => {
    mockResult = { kind: "full", content: "   \n", size: 4, lineEnding: "lf" }
    render(<MarkdownPreview path="/w/empty.md" />)
    await waitFor(() => expect(screen.getByTestId("markdown-preview-empty")).toBeTruthy())
})

test("limited grade 不建輪詢 interval（不重複 stringify）（W4）", async () => {
    vi.useFakeTimers({ shouldAdvanceTime: true })
    mockResult = { kind: "limited", content: "# big", size: 50_000_000, lineEnding: "lf" }
    render(<MarkdownPreview path="/w/big.md" />)
    await screen.findByTestId("markdown-preview-downgrade")
    vi.mocked(getView).mockClear()
    await vi.advanceTimersByTimeAsync(1300)
    expect(getView).not.toHaveBeenCalled()
})

test("點擊 http 連結 preventDefault 並改用 openUrl 外開（W10）", async () => {
    mockResult = { kind: "full", content: "[link](https://example.com)", size: 20, lineEnding: "lf" }
    render(<MarkdownPreview path="/w/l.md" />)
    const anchor = await screen.findByText("link")
    const notPrevented = fireEvent.click(anchor)
    expect(notPrevented).toBe(false)
    expect(vi.mocked(openUrl)).toHaveBeenCalledWith("https://example.com")
})

test("openUrl reject 走 .catch、不造成未捕捉錯誤（R2-9）", async () => {
    // openUrl 失敗（capability 未涵蓋／OS 無 handler）不得逸出：source 的 .catch
    // 吞掉 rejection；若無 .catch，vitest 會將 unhandled rejection 判為失敗。
    vi.mocked(openUrl).mockRejectedValueOnce(new Error("no handler"))
    mockResult = { kind: "full", content: "[link](https://example.com)", size: 20, lineEnding: "lf" }
    render(<MarkdownPreview path="/w/l.md" />)
    const anchor = await screen.findByText("link")
    expect(() => fireEvent.click(anchor)).not.toThrow()
    expect(vi.mocked(openUrl)).toHaveBeenCalledWith("https://example.com")
    await act(async () => {
        await Promise.resolve()
    })
})

test("preview 是 render container 內的 in-flow aside，不使用 portal/fixed overlay", async () => {
    mockResult = { kind: "full", content: "# x", size: 3, lineEnding: "lf" }
    const { container } = render(<MarkdownPreview path="/w/r.md" />)
    const panel = await screen.findByRole("complementary", { name: "Markdown preview" })
    expect(panel.tagName).toBe("ASIDE")
    expect(container.contains(panel)).toBe(true)
    expect(panel.className).not.toContain("fixed")
    expect(panel.className).not.toContain("w-[46vw]")
})

test("getDocument reject 顯示錯誤態、不逸出未捕捉錯誤（R3-7）", async () => {
    vi.mocked(getDocument).mockRejectedValueOnce(new Error("file gone"))
    render(<MarkdownPreview path="/w/gone.md" />)
    await screen.findByTestId("markdown-preview-error")
    expect(screen.queryByTestId("markdown-preview-downgrade")).toBeNull()
})

test("父 re-render 內容未變時不重解／重 sanitize（R4-1）", async () => {
    mockResult = { kind: "full", content: "# Memo\n\ntext", size: 12, lineEnding: "lf" }
    const { rerender } = render(<MarkdownPreview path="/w/m.md" />)
    await screen.findByRole("heading", { level: 1 })
    const base = __renderMarkdownCallCount()
    rerender(<MarkdownPreview path="/w/m.md" />)
    rerender(<MarkdownPreview path="/w/m.md" />)
    expect(__renderMarkdownCallCount()).toBe(base)
})

test("內容未變時輪詢不重複 doc.toString（R4-3）", async () => {
    vi.useFakeTimers({ shouldAdvanceTime: true })
    const toString = vi.fn(() => "# stable")
    const doc = { toString }
    vi.mocked(getView).mockImplementation(
        () => ({ state: { doc } }) as unknown as ReturnType<typeof getView>
    )
    mockResult = { kind: "full", content: "# stable", size: 8, lineEnding: "lf" }
    render(<MarkdownPreview path="/w/s.md" />)
    await screen.findByRole("heading", { level: 1 })
    toString.mockClear()
    await act(async () => {
        await vi.advanceTimersByTimeAsync(2000)
    })
    expect(toString).not.toHaveBeenCalled()
})

test("外部 reload 跨 10MB 邊界 full→tooLarge 更新 kind、停用渲染（R4-5）", async () => {
    vi.useFakeTimers({ shouldAdvanceTime: true })
    let live = "# small"
    vi.mocked(getView).mockImplementation(() => fakeView(() => live))
    mockResult = { kind: "full", content: "# small", size: 100, lineEnding: "lf" }
    render(<MarkdownPreview path="/w/g.md" />)
    await screen.findByRole("heading", { level: 1 })
    // 模擬外部 reload：檔案跨界變 tooLarge → 快取 kind 改變。
    mockResult = { kind: "tooLarge", size: 20_000_000 }
    live = "x"
    await act(async () => {
        await vi.advanceTimersByTimeAsync(500)
    })
    expect(screen.getByTestId("markdown-preview-downgrade")).toBeTruthy()
    expect(screen.queryByRole("heading", { level: 1 })).toBeNull()
})

test("輪詢 tick 的 getDocument reject 不逸出未捕捉錯誤、下一 tick 恢復（R5-1）", async () => {
    vi.useFakeTimers({ shouldAdvanceTime: true })
    let live = "# ok"
    vi.mocked(getView).mockImplementation(() => fakeView(() => live))
    mockResult = { kind: "full", content: "# ok", size: 4, lineEnding: "lf" }
    render(<MarkdownPreview path="/w/d.md" />)
    await screen.findByRole("heading", { level: 1 })

    // 外部刪檔 → reloadDocument 清快取 → 下一 tick getDocument 走 openFile reject。
    vi.mocked(getDocument).mockRejectedValueOnce(new Error("gone"))
    await act(async () => {
        await vi.advanceTimersByTimeAsync(500)
    })
    // 元件不崩、仍顯示原內容（tick 級靜默，loadError 語意留給 init 路徑）。
    expect(screen.getByRole("heading", { level: 1 })).toBeTruthy()
    expect(screen.queryByTestId("markdown-preview-error")).toBeNull()

    // 下一 tick 檔案恢復可讀 → 內容更新。
    live = "# back"
    await act(async () => {
        await vi.advanceTimersByTimeAsync(500)
    })
    expect(screen.getByText("back")).toBeTruthy()
})

test("full 檔輪詢中注入超長行顯示降級、移除後自動恢復（R2-2）", async () => {
    vi.useFakeTimers({ shouldAdvanceTime: true })
    let live = "# Title"
    vi.mocked(getView).mockImplementation(() => fakeView(() => live))
    mockResult = { kind: "full", content: "# Title", size: 7, lineEnding: "lf" }
    render(<MarkdownPreview path="/w/r.md" />)
    await screen.findByRole("heading", { level: 1 })

    // 注入 >10000 字元單行 → content-derived grade 變 veryLongLine → 顯示降級。
    live = "x".repeat(10_001)
    await act(async () => {
        await vi.advanceTimersByTimeAsync(500)
    })
    expect(screen.getByTestId("markdown-preview-downgrade")).toBeTruthy()

    // 移除長行後輪詢仍在（kind 仍 full）→ 自動恢復渲染，無需 remount。
    live = "# Back"
    await act(async () => {
        await vi.advanceTimersByTimeAsync(500)
    })
    expect(screen.getByRole("heading", { level: 1 })).toBeTruthy()
    expect(screen.queryByTestId("markdown-preview-downgrade")).toBeNull()
})

test("veryLongLine 期間不做白費的全量渲染、長行移除後恢復（R6-3）", async () => {
    vi.useFakeTimers({ shouldAdvanceTime: true })
    let live = "# Title"
    vi.mocked(getView).mockImplementation(() => fakeView(() => live))
    mockResult = { kind: "full", content: "# Title", size: 7, lineEnding: "lf" }
    render(<MarkdownPreview path="/w/vll.md" />)
    await screen.findByRole("heading", { level: 1 })

    // 注入 >10000 字元單行 → grade 變 veryLongLine → 顯示 downgrade：期間即使
    // 每 tick content 變動，也不得對整個 buffer 執行 renderMarkdown（輸出被丟棄）。
    const before = __renderMarkdownCallCount()
    live = "x".repeat(10_001)
    await act(async () => {
        await vi.advanceTimersByTimeAsync(500)
    })
    expect(screen.getByTestId("markdown-preview-downgrade")).toBeTruthy()
    live = "y".repeat(10_002)
    await act(async () => {
        await vi.advanceTimersByTimeAsync(500)
    })
    expect(__renderMarkdownCallCount()).toBe(before)

    // 長行移除後 grade 回 full → renderMarkdown 重新被呼叫、preview 正常渲染。
    live = "# Back"
    await act(async () => {
        await vi.advanceTimersByTimeAsync(500)
    })
    expect(__renderMarkdownCallCount()).toBeGreaterThan(before)
    expect(screen.getByRole("heading", { level: 1 })).toBeTruthy()
    expect(screen.queryByTestId("markdown-preview-downgrade")).toBeNull()
})

test("切換 workspace 清空 preview 開關狀態（W8）", () => {
    useMarkdownPreviewStore.setState({ openPaths: { "/w/a.md": true } })
    useWorkspaceStore.getState().setWorkspace("/w2-" + Math.random())
    expect(useMarkdownPreviewStore.getState().openPaths).toEqual({})
})

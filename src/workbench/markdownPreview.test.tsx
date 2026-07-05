import { afterEach, expect, test, vi } from "vitest"
import { act, cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react"

import type { OpenFileResult } from "../lib/types"
import { getView } from "../editor/viewRegistry"
import { getDocument } from "../editor/documentRegistry"
import { openUrl } from "@tauri-apps/plugin-opener"
import { useWorkspaceStore } from "../state/workspaceStore"

// Mutable stand-in for the editor buffer the preview reads. Each test seeds it
// before rendering, mirroring the EditorPane.test.tsx documentRegistry mock.
let mockResult: OpenFileResult = { kind: "full", content: "", size: 0 }
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

// --- renderMarkdown（純函式：markdown-it 渲染＋DOMPurify sanitize） ---

test("renderMarkdown 產出 heading／list／code block 結構", () => {
    const html = renderMarkdown("# 標題\n\n- 一\n- 二\n\n```js\nconst x = 1\n```")
    expect(html).toContain("<h1>")
    expect(html).toContain("<ul>")
    expect(html).toContain("<li>")
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
    expect(legit).toContain("<h1>")
    expect(legit).toContain("<code")
})

test("renderMarkdown 移除 class 屬性（防 Tailwind utility 重建 overlay）（R11-1）", () => {
    const html = renderMarkdown('<div class="fixed inset-0 z-50 opacity-0">x</div>')
    expect(html).not.toContain("class=")
    // 合法語法（heading／list／code fence／table）仍正確渲染——樣式靠標籤選擇器。
    const legit = renderMarkdown(
        "# H\n\n- a\n- b\n\n```js\nx\n```\n\n| a | b |\n|---|---|\n| 1 | 2 |"
    )
    expect(legit).toContain("<h1>")
    expect(legit).toContain("<li>")
    expect(legit).toContain("<code")
    expect(legit).toContain("<table>")
})

test("renderMarkdown 移除 image map（map／area／usemap）（R3-1）", () => {
    const html = renderMarkdown(
        '<img src=x usemap="#m"><map name="m"><area href="https://evil.test" shape="rect" coords="0,0,9,9"></map>'
    )
    expect(html).not.toContain("<map")
    expect(html).not.toContain("<area")
    expect(html).not.toContain("usemap")
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
    mockResult = { kind: "full", content: "# Title\n\n- one\n- two", size: 20 }
    render(<MarkdownPreview path="/w/r.md" />)
    await waitFor(() => expect(screen.getByRole("heading", { level: 1 })).toBeTruthy())
    expect(screen.getByText("one")).toBeTruthy()
})

test("limited grade 顯示降級提示、不渲染內文", async () => {
    mockResult = { kind: "limited", content: "# 很大的檔案", size: 99_999_999 }
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
    mockResult = { kind: "full", content: "   \n", size: 4 }
    render(<MarkdownPreview path="/w/empty.md" />)
    await waitFor(() => expect(screen.getByTestId("markdown-preview-empty")).toBeTruthy())
})

test("limited grade 不建輪詢 interval（不重複 stringify）（W4）", async () => {
    vi.useFakeTimers({ shouldAdvanceTime: true })
    mockResult = { kind: "limited", content: "# big", size: 50_000_000 }
    render(<MarkdownPreview path="/w/big.md" />)
    await screen.findByTestId("markdown-preview-downgrade")
    vi.mocked(getView).mockClear()
    await vi.advanceTimersByTimeAsync(1300)
    expect(getView).not.toHaveBeenCalled()
})

test("點擊 http 連結 preventDefault 並改用 openUrl 外開（W10）", async () => {
    mockResult = { kind: "full", content: "[link](https://example.com)", size: 20 }
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
    mockResult = { kind: "full", content: "[link](https://example.com)", size: 20 }
    render(<MarkdownPreview path="/w/l.md" />)
    const anchor = await screen.findByText("link")
    expect(() => fireEvent.click(anchor)).not.toThrow()
    expect(vi.mocked(openUrl)).toHaveBeenCalledWith("https://example.com")
    await act(async () => {
        await Promise.resolve()
    })
})

test("overlay bottom 偏移避開底部 StatusBar（R3-6）", async () => {
    mockResult = { kind: "full", content: "# x", size: 3 }
    render(<MarkdownPreview path="/w/r.md" />)
    const panel = await screen.findByRole("complementary", { name: "Markdown preview" })
    expect(panel.className).not.toContain("bottom-0")
    expect(panel.className).toContain("bottom-[30px]")
})

test("getDocument reject 顯示錯誤態、不逸出未捕捉錯誤（R3-7）", async () => {
    vi.mocked(getDocument).mockRejectedValueOnce(new Error("file gone"))
    render(<MarkdownPreview path="/w/gone.md" />)
    await screen.findByTestId("markdown-preview-error")
    expect(screen.queryByTestId("markdown-preview-downgrade")).toBeNull()
})

test("父 re-render 內容未變時不重解／重 sanitize（R4-1）", async () => {
    mockResult = { kind: "full", content: "# Memo\n\ntext", size: 12 }
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
    mockResult = { kind: "full", content: "# stable", size: 8 }
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
    mockResult = { kind: "full", content: "# small", size: 100 }
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
    mockResult = { kind: "full", content: "# ok", size: 4 }
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
    mockResult = { kind: "full", content: "# Title", size: 7 }
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
    mockResult = { kind: "full", content: "# Title", size: 7 }
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

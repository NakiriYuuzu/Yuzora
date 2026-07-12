import { memo, useEffect, useMemo, useRef, useState } from "react"
import MarkdownIt from "markdown-it"
import DOMPurify from "dompurify"
import { create } from "zustand"
import { openUrl } from "@tauri-apps/plugin-opener"

import { getDocument } from "../editor/documentRegistry"
import { getView } from "../editor/viewRegistry"
import { useWorkspaceStore } from "../state/workspaceStore"
import { fileGradeOf } from "../lib/types"
import type { OpenFileResult } from "../lib/types"
import {
    collectPreviewAnchors,
    createScrollSyncCoordinator,
    readEditorViewportTopLine,
    writeEditorViewportTopLine
} from "./markdownScrollSync"
import type { ScrollSyncCoordinator, SourceAnchor } from "./markdownScrollSync"

// A4 裁決：渲染器＝markdown-it，sanitizer＝DOMPurify。html:true 讓原始 HTML
// 通過 markdown-it，交由 DOMPurify 白名單過濾（XSS 防護的唯一守門）。
const md = new MarkdownIt({ html: true, linkify: true, breaks: false })
const SOURCE_LINE_ATTR = "data-yz-source-line"
const SOURCE_ANCHOR_ATTR = "data-yz-source-anchor"

function isHtmlWhitespace(character: string): boolean {
    return character === " " || character === "\t" || character === "\n"
        || character === "\f" || character === "\r"
}

function stripReservedSourceAttributesFromStartTag(
    html: string,
    tagStart: number
): { content: string; end: number } | null {
    if (!/[a-z]/i.test(html[tagStart + 1] ?? "")) return null

    let cursor = tagStart + 2
    while (
        cursor < html.length
        && !isHtmlWhitespace(html[cursor])
        && html[cursor] !== "/"
        && html[cursor] !== ">"
    ) cursor++

    const removals: Array<[number, number]> = []
    while (cursor < html.length && html[cursor] !== ">") {
        while (isHtmlWhitespace(html[cursor] ?? "")) cursor++
        if (html[cursor] === "/") {
            cursor++
            continue
        }
        if (html[cursor] === ">" || cursor >= html.length) break

        const attributeStart = cursor
        cursor++
        while (
            cursor < html.length
            && !isHtmlWhitespace(html[cursor])
            && html[cursor] !== "/"
            && html[cursor] !== "="
            && html[cursor] !== ">"
        ) cursor++
        const attributeName = html.slice(attributeStart, cursor).toLowerCase()
        const attributeNameEnd = cursor

        while (isHtmlWhitespace(html[cursor] ?? "")) cursor++
        let attributeEnd = attributeNameEnd
        if (html[cursor] === "=") {
            cursor++
            while (isHtmlWhitespace(html[cursor] ?? "")) cursor++
            const quote = html[cursor]
            if (quote === '"' || quote === "'") {
                cursor++
                while (cursor < html.length && html[cursor] !== quote) cursor++
                if (html[cursor] === quote) cursor++
            } else {
                // In HTML's unquoted-value state, slash is content rather than
                // a separator. Only whitespace or the closing bracket ends it.
                while (
                    cursor < html.length
                    && !isHtmlWhitespace(html[cursor])
                    && html[cursor] !== ">"
                ) cursor++
            }
            attributeEnd = cursor
        }

        if (attributeName === SOURCE_LINE_ATTR || attributeName === SOURCE_ANCHOR_ATTR) {
            let removalStart = attributeStart
            while (
                removalStart > tagStart + 1
                && (isHtmlWhitespace(html[removalStart - 1]) || html[removalStart - 1] === "/")
            ) removalStart--
            const previous = removals.at(-1)
            if (previous && removalStart <= previous[1]) previous[1] = attributeEnd
            else removals.push([removalStart, attributeEnd])
        }
    }

    if (html[cursor] === ">") cursor++
    if (removals.length === 0) return { content: html.slice(tagStart, cursor), end: cursor }

    let content = ""
    let keptFrom = tagStart
    for (const [start, end] of removals) {
        content += html.slice(keptFrom, start)
        keptFrom = end
    }
    content += html.slice(keptFrom, cursor)
    return { content, end: cursor }
}

function stripReservedSourceAttributes(html: string): string {
    let output = ""
    let cursor = 0
    while (cursor < html.length) {
        const tagStart = html.indexOf("<", cursor)
        if (tagStart < 0) return output + html.slice(cursor)
        output += html.slice(cursor, tagStart)

        if (html.startsWith("<!--", tagStart)) {
            const commentEnd = html.indexOf("-->", tagStart + 4)
            if (commentEnd < 0) return output + html.slice(tagStart)
            output += html.slice(tagStart, commentEnd + 3)
            cursor = commentEnd + 3
            continue
        }

        const startTag = stripReservedSourceAttributesFromStartTag(html, tagStart)
        if (startTag) {
            output += startTag.content
            cursor = startTag.end
            continue
        }

        let tagEnd = tagStart + 1
        let quote = ""
        for (; tagEnd < html.length; tagEnd++) {
            const character = html[tagEnd]
            if (quote) {
                if (character === quote) quote = ""
            } else if (character === '"' || character === "'") {
                quote = character
            } else if (character === ">") {
                break
            }
        }
        if (tagEnd >= html.length) return output + html.slice(tagStart)

        const tag = html.slice(tagStart, tagEnd + 1)
        output += tag
        cursor = tagEnd + 1
    }
    return output
}

// html_block's stock renderer returns raw content and ignores token attrs. A
// zero-height in-flow sentinel gives that block the same trusted marker contract
// as normal opening/self-closing tokens without wrapping or changing its HTML.
md.renderer.rules.html_block = (tokens, index, _options, _env, renderer) => {
    const token = tokens[index]
    return `<span${renderer.renderAttrs(token)} aria-hidden="true"></span>${token.content}`
}

type MarkdownToken = ReturnType<typeof md.parse>[number]

function annotateSourceTokens(tokens: MarkdownToken[]): void {
    for (const token of tokens) {
        // Raw Markdown HTML is allowed, but the marker namespace is reserved for
        // renderer-generated attrs. Strip spoofed attrs before rendering/sanitize.
        if (token.type === "html_block" || token.type === "html_inline") {
            token.content = stripReservedSourceAttributes(token.content)
        }
        if (token.children) annotateSourceTokens(token.children)
        if (!token.block || !token.map) continue
        const line = token.map[0] + 1
        token.attrSet(SOURCE_LINE_ATTR, String(line))
        token.attrSet(SOURCE_ANCHOR_ATTR, "block")
    }
}

// 渲染呼叫計數：供測試斷言 memo 生效（父 re-render／內容未變時不重解）（R4-1）。
let renderMarkdownCalls = 0
export function __renderMarkdownCallCount(): number {
    return renderMarkdownCalls
}

export function renderMarkdown(src: string): string {
    renderMarkdownCalls++
    const tokens = md.parse(src, {})
    annotateSourceTokens(tokens)
    // FORBID_ATTR target／usemap：連結一律不得帶 target（防 window.open 突破
    // webview）；usemap 讓 image map 導航（R3-1）；style 屬性＝inline CSS，可
    // position:fixed;inset:0 全域 overlay（clickjacking／偽裝）或 background:url()
    // 發遠端 beacon（R10-2）；class＝Tailwind v4 已把 overlay utility（fixed/
    // inset-0/z-50/opacity-0）編進 bundle，class 通道可重建同款全域 overlay
    // （R11-1）。prose 樣式全用標籤選擇器，移除 class 對顯示零副作用。
    // javascript:/data: 協定由 DOMPurify 預設清除。
    // FORBID_TAGS：form／表單控件可提交導航離開 webview（R2-7）；map／area 為
    // image map 導航元素，其 closest("a") 為 null 會逃逸 anchor 攔截（R3-1）；
    // style 跟在內容後可存活 sanitize；即使 preview 是 in-flow pane，全域 CSS 仍
    // 可藏匿整個 app（R9-1）。清單與 lspManager.ts 逐字同步。
    return DOMPurify.sanitize(md.renderer.render(tokens, md.options, {}), {
        FORBID_ATTR: ["target", "usemap", "style", "class"],
        FORBID_TAGS: ["form", "input", "button", "select", "textarea", "dialog", "map", "area", "style"]
    })
}

export function isMarkdownPath(name: string): boolean {
    const ext = name.includes(".") ? name.split(".").pop()!.toLowerCase() : ""
    return ext === "md" || ext === "markdown"
}

// Preview 開關狀態（per path）。TabBar 與 editor group 內的 split view 共用。
interface MarkdownPreviewState {
    openPaths: Record<string, boolean>
    toggle: (path: string) => void
    close: (path: string) => void
    reset: () => void
    isOpen: (path: string) => boolean
}

export const useMarkdownPreviewStore = create<MarkdownPreviewState>((set, get) => ({
    openPaths: {},
    toggle: (path) =>
        set((s) => ({ openPaths: { ...s.openPaths, [path]: !s.openPaths[path] } })),
    close: (path) => set((s) => ({ openPaths: { ...s.openPaths, [path]: false } })),
    reset: () => set({ openPaths: {} }),
    isOpen: (path) => !!get().openPaths[path]
}))

// workspace 切換清空開關狀態，避免無界累積與跨專案殘留（W8）。
useWorkspaceStore.subscribe((s, prev) => {
    if (s.workspacePath !== prev.workspacePath) useMarkdownPreviewStore.getState().reset()
})

function bufferContent(path: string, result: OpenFileResult): string {
    // 優先讀 CodeMirror live doc（未存檔編輯），退回 registry 內容。
    const live = getView(path)?.state.doc.toString()
    if (live !== undefined) return live
    if (result.kind === "full" || result.kind === "limited" || result.kind === "nonUtf8Readonly") {
        return result.content
    }
    return ""
}

export const MarkdownPreview = memo(function MarkdownPreview({
    path,
    style
}: {
    path: string
    style?: React.CSSProperties
}) {
    const [result, setResult] = useState<OpenFileResult | null>(null)
    const [content, setContent] = useState("")
    const [loadError, setLoadError] = useState(false)
    const close = useMarkdownPreviewStore((s) => s.close)
    // 上次讀取的 CM6 doc 參考（immutable，每 transaction 換新物件）：identity
    // 比對省去未變時對至多 10MB doc 的全量 toString（R4-3）。
    const lastDocRef = useRef<unknown>(null)
    const previewBodyRef = useRef<HTMLDivElement>(null)
    const coordinatorRef = useRef<ScrollSyncCoordinator | null>(null)
    const rebuildAnchorsRef = useRef<((preservedSourceLine?: number) => void) | null>(null)
    const pendingSourceLineRef = useRef<number | null>(null)
    const documentLineCountRef = useRef(1)

    useEffect(() => {
        let disposed = false
        setLoadError(false)
        void getDocument(path)
            .then((entry) => {
                if (disposed) return
                setResult(entry.result)
                lastDocRef.current = getView(path)?.state.doc ?? null
                setContent(bufferContent(path, entry.result))
            })
            // openFile reject（檔案被刪／權限）：改顯示錯誤態，避免永卡「載入中」
            // 與 unhandled rejection（R3-7）。
            .catch(() => {
                if (!disposed) setLoadError(true)
            })
        return () => {
            disposed = true
        }
    }, [path])

    const grade = useMemo(() => (result ? fileGradeOf(result, content) : null), [result, content])

    // 即時更新（debounce by poll）：定時讀 live doc，內容變動才 setState。
    // 條件看 kind-derived grade（result.kind），不看 content-derived grade：
    // full 檔即持續輪詢——即使暫態貼入超長行讓 content-derived grade 變
    // veryLongLine（渲染分支顯示降級），輪詢不停，長行刪除後自動恢復（R2-2、W4）。
    useEffect(() => {
        if (result?.kind !== "full") return
        const id = setInterval(() => {
            // R4-5：外部 reload 可能跨 10MB 邊界改 kind（full↔tooLarge）；每 tick
            // 重讀快取比對 kind，變動即 setResult（full→tooLarge 時同步停用渲染守衛
            // 並終止輪詢）。documentRegistry 無同步 peek／更新事件，故經 getDocument
            // 讀快取引用。
            void getDocument(path)
                .then((entry) => {
                    if (entry.result.kind !== result.kind) {
                        setResult(entry.result)
                        return
                    }
                    // R4-3：doc identity 未變則跳過 toString（CM6 doc immutable）。
                    const doc = getView(path)?.state.doc
                    if (doc === undefined || doc === lastDocRef.current) return
                    lastDocRef.current = doc
                    const live = doc.toString()
                    pendingSourceLineRef.current = coordinatorRef.current?.snapshotSourceLine() ?? null
                    setContent((current) => current === live ? current : live)
                })
                // 外部刪檔清快取後，tick 的 getDocument 走 openFile reject——tick 級
                // 靜默即可（loadError 語意留給 init 路徑；下一 tick 自然重試）（R5-1）。
                .catch(() => {})
        }, 400)
        return () => clearInterval(id)
    }, [path, result?.kind])

    // preview 連結一律不得讓 webview 導航離開（會失去整個 editor 狀態）：攔截
    // anchor，http/https 改用系統瀏覽器外開，其他 scheme（含相對路徑）僅擋掉（W10）。
    function onPreviewClick(e: React.MouseEvent<HTMLElement>) {
        const anchor = (e.target as HTMLElement).closest("a")
        if (!anchor) return
        e.preventDefault()
        const href = anchor.getAttribute("href") ?? ""
        // .catch：opener capability 未涵蓋或 OS 無 handler 時避免 unhandled rejection（R2-9）。
        if (/^https?:\/\//i.test(href)) void openUrl(href).catch(() => {})
    }

    // 渲染＋sanitize 只隨 grade／content 變動而重算——避免父 re-render（markDirty
    // 每鍵重建 groups → TabBar re-render）時對未變內容全文重解（R4-1）。閘門與
    // 顯示閘（grade === "full"）一致：非 full（limited 數十 MB／暫態 veryLongLine
    // 顯示 downgrade）不進渲染，回 null——省下輸出被丟棄的白費全量渲染（R6-3）。
    const html = useMemo(
        () => (grade === "full" ? renderMarkdown(content) : null),
        [grade, content]
    )
    const documentLineCount = useMemo(() => content.split(/\r\n?|\n/).length, [content])
    const syncEnabled = grade === "full" && content.trim() !== ""

    useEffect(() => {
        documentLineCountRef.current = documentLineCount
    }, [documentLineCount])

    useEffect(() => {
        if (!syncEnabled) return
        const preview = previewBodyRef.current
        if (!preview) return

        let disposed = false
        let attachedView: ReturnType<typeof getView>
        let detach = () => {}

        function attachCurrentView() {
            if (disposed) return
            const view = getView(path)
            if (view === attachedView && coordinatorRef.current) return

            detach()
            detach = () => {}
            attachedView = view
            coordinatorRef.current = null
            rebuildAnchorsRef.current = null
            // Tests and async mount windows may expose a doc-only registry stand-in
            // before the actual EditorView scroll surface is ready.
            if (
                !view
                || !(view.scrollDOM instanceof HTMLElement)
                || typeof view.lineBlockAtHeight !== "function"
                || typeof view.lineBlockAt !== "function"
            ) return

            let anchors: SourceAnchor[] = collectPreviewAnchors(
                preview!,
                documentLineCountRef.current
            )
            const coordinator = createScrollSyncCoordinator({
                editor: {
                    subscribeScroll: (listener) => {
                        view.scrollDOM.addEventListener("scroll", listener, { passive: true })
                        return () => view.scrollDOM.removeEventListener("scroll", listener)
                    },
                    readOffset: () => Number.isFinite(view.scrollDOM.scrollTop)
                        ? view.scrollDOM.scrollTop
                        : null,
                    readSourceLine: () => readEditorViewportTopLine(view),
                    writeSourceLine: (line) => writeEditorViewportTopLine(view, line)
                },
                preview: {
                    subscribeScroll: (listener) => {
                        preview!.addEventListener("scroll", listener, { passive: true })
                        return () => preview!.removeEventListener("scroll", listener)
                    },
                    readOffset: () => Number.isFinite(preview!.scrollTop)
                        ? preview!.scrollTop
                        : null,
                    writeOffset: (offset) => {
                        const maximum = Math.max(0, preview!.scrollHeight - preview!.clientHeight)
                        preview!.scrollTop = Math.min(maximum, Math.max(0, offset))
                        return Number.isFinite(preview!.scrollTop) ? preview!.scrollTop : null
                    }
                },
                getAnchors: () => anchors
            })

            function rebuildAnchors(preservedSourceLine = coordinator.snapshotSourceLine() ?? undefined) {
                anchors = collectPreviewAnchors(preview!, documentLineCountRef.current)
                coordinator.resync(preservedSourceLine)
            }

            const observer = new ResizeObserver(() => rebuildAnchors())
            observer.observe(preview!)
            const onImageLoad = (event: Event) => {
                if (event.target instanceof HTMLImageElement) rebuildAnchors()
            }
            preview!.addEventListener("load", onImageLoad, true)
            const fontSet = document.fonts as FontFaceSet | undefined
            let fontSubscriptionActive = true
            const onFontLayoutChange = () => {
                if (fontSubscriptionActive) rebuildAnchors()
            }
            fontSet?.addEventListener("loadingdone", onFontLayoutChange)
            void fontSet?.ready.then(onFontLayoutChange)
            coordinatorRef.current = coordinator
            rebuildAnchorsRef.current = rebuildAnchors
            coordinator.resync()

            detach = () => {
                fontSubscriptionActive = false
                fontSet?.removeEventListener("loadingdone", onFontLayoutChange)
                observer.disconnect()
                preview!.removeEventListener("load", onImageLoad, true)
                coordinator.destroy()
                if (coordinatorRef.current === coordinator) coordinatorRef.current = null
                if (rebuildAnchorsRef.current === rebuildAnchors) rebuildAnchorsRef.current = null
            }
        }

        // EditorPane registers asynchronously. Re-check on the same 400ms cadence
        // as live content polling; preview remains independently usable meanwhile.
        attachCurrentView()
        const attachInterval = setInterval(attachCurrentView, 400)
        return () => {
            disposed = true
            clearInterval(attachInterval)
            detach()
            coordinatorRef.current = null
            rebuildAnchorsRef.current = null
            pendingSourceLineRef.current = null
        }
    }, [path, syncEnabled])

    useEffect(() => {
        if (!syncEnabled) return
        const preservedSourceLine = pendingSourceLineRef.current
            ?? coordinatorRef.current?.snapshotSourceLine()
            ?? undefined
        pendingSourceLineRef.current = null
        rebuildAnchorsRef.current?.(preservedSourceLine)
    }, [documentLineCount, html, syncEnabled])

    const statusClass = "flex flex-1 items-center justify-center p-[24px] text-[12.5px] text-(--ink-4)"
    let body: React.ReactNode
    if (loadError) {
        body = (
            <div className={statusClass} data-testid="markdown-preview-error">
                無法讀取檔案，Markdown 預覽已停用。
            </div>
        )
    } else if (!result) {
        body = <div className={statusClass}>載入中…</div>
    } else if (grade !== "full") {
        body = (
            <div className={statusClass} data-testid="markdown-preview-downgrade">
                檔案過大或無法解析，Markdown 預覽已停用。
            </div>
        )
    } else if (content.trim() === "") {
        body = (
            <div className={statusClass} data-testid="markdown-preview-empty">
                沒有可預覽的內容。
            </div>
        )
    } else {
        body = (
            <div
                ref={previewBodyRef}
                data-testid="markdown-preview-body"
                className="markdown-preview-body min-h-0 flex-1 overflow-y-auto px-[28px] py-[20px] text-[14px] leading-[1.7] text-(--ink-1)"
                dangerouslySetInnerHTML={{ __html: html ?? "" }}
            />
        )
    }

    return (
        <aside
            role="complementary"
            aria-label="Markdown preview"
            onClick={onPreviewClick}
            style={style}
            className="markdown-preview yzs flex min-h-0 min-w-0 flex-col overflow-hidden bg-(--paper-0)"
        >
            <MarkdownPreviewProse />
            <div className="flex h-[38px] shrink-0 items-center justify-between border-b border-(--line-1) px-[14px]">
                <span className="text-[11px] font-semibold tracking-wide text-(--ink-3) uppercase">
                    Preview
                </span>
                <button
                    type="button"
                    className="flex size-[22px] items-center justify-center rounded-[6px] text-(--ink-3) transition-colors hover:bg-(--paper-3) hover:text-(--ink-0)"
                    aria-label="Close preview"
                    title="關閉預覽"
                    onClick={() => close(path)}
                >
                    <svg
                        width="12"
                        height="12"
                        viewBox="0 0 24 24"
                        fill="none"
                        stroke="currentColor"
                        strokeWidth="2.2"
                        strokeLinecap="round"
                        aria-hidden="true"
                    >
                        <path d="M18 6 6 18M6 6l12 12" />
                    </svg>
                </button>
            </div>
            {body}
        </aside>
    )
})

// Rendered HTML comes from dangerouslySetInnerHTML, so Tailwind can't reach it.
// A scoped style block gives the sanitized markdown legible prose styling
// without adding a CSS file (out of this task's scope).
// contain:paint 在 .markdown-preview-body 上建立 containing block——不可信 HTML
// 內任何 position:fixed 子元素只相對 preview 內容區定位，無法覆蓋 editor。這是對
// CSS-overlay 逃逸的根因防禦（不依賴列舉 style/class 等個別屬性通道）（R11-1b）。
// jsdom 測不到 layout 定位，實機效果歸 T15 gui-acceptance。
function MarkdownPreviewProse() {
    return (
        <style>{`
.markdown-preview-body{contain:paint}
.markdown-preview-body span[data-yz-source-anchor="block"]{display:block;height:0;overflow:hidden}
.markdown-preview-body h1{font-size:1.7em;font-weight:700;margin:.6em 0 .4em;line-height:1.25}
.markdown-preview-body h2{font-size:1.4em;font-weight:700;margin:.6em 0 .35em;line-height:1.3}
.markdown-preview-body h3{font-size:1.15em;font-weight:600;margin:.5em 0 .3em}
.markdown-preview-body p{margin:.5em 0}
.markdown-preview-body ul,.markdown-preview-body ol{margin:.5em 0;padding-left:1.5em}
.markdown-preview-body ul{list-style:disc}
.markdown-preview-body ol{list-style:decimal}
.markdown-preview-body li{margin:.2em 0}
.markdown-preview-body a{color:var(--yz-accent-ink);text-decoration:underline}
.markdown-preview-body code{font-family:var(--font-mono,monospace);font-size:.88em;background:var(--paper-3);border-radius:4px;padding:.1em .35em}
.markdown-preview-body pre{background:var(--paper-3);border-radius:8px;padding:12px 14px;overflow-x:auto;margin:.6em 0}
.markdown-preview-body pre code{background:none;padding:0}
.markdown-preview-body blockquote{border-left:3px solid var(--line-1);margin:.6em 0;padding:.1em 0 .1em 14px;color:var(--ink-3)}
.markdown-preview-body table{border-collapse:collapse;margin:.6em 0}
.markdown-preview-body th,.markdown-preview-body td{border:1px solid var(--line-1);padding:5px 10px}
.markdown-preview-body img{max-width:100%}
.markdown-preview-body hr{border:none;border-top:1px solid var(--line-1);margin:1em 0}
`}</style>
    )
}

import { memo, useEffect, useMemo, useRef, useState } from "react"
import { createPortal } from "react-dom"
import MarkdownIt from "markdown-it"
import DOMPurify from "dompurify"
import { create } from "zustand"
import { openUrl } from "@tauri-apps/plugin-opener"

import { getDocument } from "../editor/documentRegistry"
import { getView } from "../editor/viewRegistry"
import { useWorkspaceStore } from "../state/workspaceStore"
import { fileGradeOf } from "../lib/types"
import type { OpenFileResult } from "../lib/types"

// A4 裁決：渲染器＝markdown-it，sanitizer＝DOMPurify。html:true 讓原始 HTML
// 通過 markdown-it，交由 DOMPurify 白名單過濾（XSS 防護的唯一守門）。
const md = new MarkdownIt({ html: true, linkify: true, breaks: false })

// 渲染呼叫計數：供測試斷言 memo 生效（父 re-render／內容未變時不重解）（R4-1）。
let renderMarkdownCalls = 0
export function __renderMarkdownCallCount(): number {
    return renderMarkdownCalls
}

export function renderMarkdown(src: string): string {
    renderMarkdownCalls++
    // FORBID_ATTR target／usemap：連結一律不得帶 target（防 window.open 突破
    // webview）；usemap 讓 image map 導航（R3-1）；style 屬性＝inline CSS，可
    // position:fixed;inset:0 全域 overlay（clickjacking／偽裝）或 background:url()
    // 發遠端 beacon（R10-2）；class＝Tailwind v4 已把 overlay utility（fixed/
    // inset-0/z-50/opacity-0）編進 bundle，class 通道可重建同款全域 overlay
    // （R11-1）。prose 樣式全用標籤選擇器，移除 class 對顯示零副作用。
    // javascript:/data: 協定由 DOMPurify 預設清除。
    // FORBID_TAGS：form／表單控件可提交導航離開 webview（R2-7）；map／area 為
    // image map 導航元素，其 closest("a") 為 null 會逃逸 anchor 攔截（R3-1）；
    // style 跟在內容後可存活 sanitize，經 portal 到 body 後全域 CSS 生效、可藏匿
    // 整個 app（R9-1）。清單與 lspManager.ts 逐字同步。
    return DOMPurify.sanitize(md.render(src), {
        FORBID_ATTR: ["target", "usemap", "style", "class"],
        FORBID_TAGS: ["form", "input", "button", "select", "textarea", "dialog", "map", "area", "style"]
    })
}

export function isMarkdownPath(name: string): boolean {
    const ext = name.includes(".") ? name.split(".").pop()!.toLowerCase() : ""
    return ext === "md" || ext === "markdown"
}

// Preview 開關狀態（per path）。TabBar 的 toggle 與 overlay 皆讀此 store，
// 避免動 EditorArea／EditorPane（本 task scope 外）。
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

export const MarkdownPreview = memo(function MarkdownPreview({ path }: { path: string }) {
    const [result, setResult] = useState<OpenFileResult | null>(null)
    const [content, setContent] = useState("")
    const [loadError, setLoadError] = useState(false)
    const close = useMarkdownPreviewStore((s) => s.close)
    // 上次讀取的 CM6 doc 參考（immutable，每 transaction 換新物件）：identity
    // 比對省去未變時對至多 10MB doc 的全量 toString（R4-3）。
    const lastDocRef = useRef<unknown>(null)

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
                    setContent((c) => (c === live ? c : live))
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
                className="markdown-preview-body min-h-0 flex-1 overflow-y-auto px-[28px] py-[20px] text-[14px] leading-[1.7] text-(--ink-1)"
                dangerouslySetInnerHTML={{ __html: html ?? "" }}
            />
        )
    }

    return createPortal(
        <div
            role="complementary"
            aria-label="Markdown preview"
            onClick={onPreviewClick}
            className="markdown-preview yzs fixed top-[44px] right-0 bottom-[30px] z-40 flex w-[46vw] min-w-[320px] flex-col border-l border-(--line-1) bg-(--paper-0) shadow-(--shadow-md)"
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
        </div>,
        document.body
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

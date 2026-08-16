import { useEffect, useLayoutEffect, useRef, useState } from "react"
import { EditorSelection, EditorState } from "@codemirror/state"
import { EditorView, lineNumbers } from "@codemirror/view"
import { syntaxHighlighting } from "@codemirror/language"
import {
    getChunks,
    goToNextChunk,
    goToPreviousChunk,
    MergeView,
    unifiedMergeView
} from "@codemirror/merge"
import { FileWarning } from "lucide-react"
import { useTranslation } from "react-i18next"

import i18n from "@/lib/i18n"
import { appHighlightStyle, appTheme } from "@/editor/cmTheme"
import { hasVeryLongLine, languageExtensionFromPath } from "@/editor/cmExtensions"
import { EmptyState } from "@/app/workbench/EmptyState"
import type { DiffContent, GradedText } from "@/lib/types"
import { Button } from "@/components/ui/button"
import { useUiStore } from "@/state/uiStore"
import {
    clampGitDiffSplitRatio,
    currentChunkIndex,
    GIT_DIFF_SPLIT_MAX,
    GIT_DIFF_SPLIT_MIN,
    GIT_DIFF_SPLIT_STEP
} from "@/workbench/git/diffPreview"

// Same theme + highlight the editor uses, so diff surfaces (background, gutters,
// tooltips) follow the app theme instead of CodeMirror's light baseTheme default.
const themeExtensions = [appTheme, syntaxHighlighting(appHighlightStyle)]

type DiffMode = "unified" | "split"

// tooLarge/binary carry no content, so they can't be diffed. Any such side
// short-circuits the whole view into a status message (spec: EmptyState,
// title「無法顯示 diff」, description by kind). Called from DiffView's render
// body, not a hook itself — reads the current language off the shared i18n
// singleton; DiffView's own useTranslation("menus") call re-renders it on
// language change.
function undisplayable(side: GradedText): string | null {
    if (side.kind === "tooLarge") return i18n.t("diffView.fileTooLarge", { ns: "menus" })
    if (side.kind === "binary") return i18n.t("diffView.binaryFile", { ns: "menus" })
    return null
}

// full/limited both carry content — limited is just a truncated slice from the
// Rust grading pipeline, so it diffs the same way.
function docOf(side: GradedText): string {
    return "content" in side ? side.content : ""
}

function navView(view: EditorView | MergeView): EditorView {
    return view instanceof MergeView ? view.b : view
}

function readChunkState(state: EditorState): { current: number; total: number } {
    const info = getChunks(state)
    const total = info?.chunks.length ?? 0
    if (total === 0) return { current: 0, total: 0 }
    const index = currentChunkIndex(state)
    return { current: Math.max(0, index), total }
}

function revealChunk(view: EditorView, index: number) {
    const info = getChunks(view.state)
    if (!info || info.chunks.length === 0) return
    const chunk = info.chunks[((index % info.chunks.length) + info.chunks.length) % info.chunks.length]
    const side = info.side ?? "b"
    const from = side === "b" ? chunk.fromB : chunk.fromA
    const to = side === "b" ? chunk.toB : chunk.toA
    view.dispatch({
        selection: { anchor: from },
        userEvent: "select.byChunk",
        effects: EditorView.scrollIntoView(EditorSelection.range(to, from), { y: "center" })
    })
}

export function DiffView({ content, mode, path }: { content: DiffContent; mode: DiffMode; path: string }) {
    const { t } = useTranslation("menus")
    const containerRef = useRef<HTMLDivElement>(null)
    const viewRef = useRef<EditorView | MergeView | null>(null)
    const draggingRef = useRef(false)
    const splitRatio = useUiStore((s) => s.gitDiffSplitRatio)
    const setSplitRatio = useUiStore((s) => s.setGitDiffSplitRatio)
    const resetSplitRatio = useUiStore((s) => s.resetGitDiffSplitRatio)
    const [chunkState, setChunkState] = useState({ current: 0, total: 0 })

    const reason = undisplayable(content.original) ?? undisplayable(content.modified)
    const original = docOf(content.original)
    const modified = docOf(content.modified)
    // Language facet lets @codemirror/merge highlight both sides (incl. deleted
    // lines via syntaxHighlightDeletions). Very long lines (minified diffs) skip
    // it to keep the parser from stalling, matching the editor's syntaxOff guard.
    const langExt =
        hasVeryLongLine(original) || hasVeryLongLine(modified) ? null : languageExtensionFromPath(path)

    useLayoutEffect(() => {
        const host = containerRef.current
        if (!host) return
        host.style.setProperty("--yz-diff-split-a", String(splitRatio))
    }, [splitRatio, mode, reason])

    useEffect(() => {
        if (reason) return
        const parent = containerRef.current
        if (!parent) return

        const langExtensions = langExt ? [langExt] : []
        const updateListener = EditorView.updateListener.of((update) => {
            if (update.selectionSet || update.docChanged) {
                setChunkState(readChunkState(update.state))
            }
        })
        let view: EditorView | MergeView
        if (mode === "split") {
            view = new MergeView({
                a: {
                    doc: original,
                    extensions: [
                        lineNumbers(),
                        ...themeExtensions,
                        ...langExtensions,
                        EditorState.readOnly.of(true),
                        updateListener
                    ]
                },
                b: {
                    doc: modified,
                    extensions: [
                        lineNumbers(),
                        ...themeExtensions,
                        ...langExtensions,
                        EditorState.readOnly.of(true),
                        updateListener
                    ]
                },
                parent
            })
        } else {
            view = new EditorView({
                doc: modified,
                extensions: [
                    lineNumbers(),
                    ...themeExtensions,
                    ...langExtensions,
                    unifiedMergeView({ original, mergeControls: false }),
                    EditorState.readOnly.of(true),
                    updateListener
                ],
                parent
            })
        }
        viewRef.current = view
        parent.style.setProperty("--yz-diff-split-a", String(useUiStore.getState().gitDiffSplitRatio))

        const syncChunks = () => {
            const active = navView(view)
            const next = readChunkState(active.state)
            setChunkState(next)
            if (next.total > 0) revealChunk(active, 0)
        }
        const frame = requestAnimationFrame(syncChunks)

        return () => {
            cancelAnimationFrame(frame)
            viewRef.current = null
            view.destroy()
        }
    }, [reason, original, modified, mode, path])

    useEffect(() => {
        if (reason) return
        function onKey(event: KeyboardEvent) {
            const host = containerRef.current
            if (!host || !(event.target instanceof Node)) return
            const surface = host.parentElement?.closest("[data-diff-surface]") ?? host.parentElement ?? host
            if (!surface.contains(event.target)) return
            const next = event.key === "F7" && !event.shiftKey || event.key === "ArrowDown" && event.altKey
            const prev = event.key === "F7" && event.shiftKey || event.key === "ArrowUp" && event.altKey
            if (!next && !prev) return
            event.preventDefault()
            stepChunk(next ? 1 : -1)
        }
        window.addEventListener("keydown", onKey)
        return () => window.removeEventListener("keydown", onKey)
    }, [reason])

    function stepChunk(dir: 1 | -1) {
        const view = viewRef.current
        if (!view) return
        const active = navView(view)
        const command = dir > 0 ? goToNextChunk : goToPreviousChunk
        command({ state: active.state, dispatch: active.dispatch })
        setChunkState(readChunkState(active.state))
    }

    function applyRatioFromClientX(clientX: number) {
        const host = containerRef.current
        if (!host) return splitRatio
        const rect = host.getBoundingClientRect()
        if (rect.width <= 0) return splitRatio
        const next = clampGitDiffSplitRatio((clientX - rect.left) / rect.width)
        host.style.setProperty("--yz-diff-split-a", String(next))
        return next
    }

    function onSeparatorPointerDown(event: React.PointerEvent<HTMLDivElement>) {
        draggingRef.current = true
        event.currentTarget.setPointerCapture(event.pointerId)
        applyRatioFromClientX(event.clientX)
    }

    function onSeparatorPointerMove(event: React.PointerEvent<HTMLDivElement>) {
        if (!draggingRef.current) return
        const next = applyRatioFromClientX(event.clientX)
        event.currentTarget.setAttribute("aria-valuenow", String(Math.round(next * 100)))
    }

    function commitSeparator(event: React.PointerEvent<HTMLDivElement>) {
        if (!draggingRef.current) return
        draggingRef.current = false
        if (event.currentTarget.hasPointerCapture(event.pointerId)) {
            event.currentTarget.releasePointerCapture(event.pointerId)
        }
        const host = containerRef.current
        const raw = host ? Number(host.style.getPropertyValue("--yz-diff-split-a")) : splitRatio
        setSplitRatio(raw)
    }

    function onSeparatorKeyDown(event: React.KeyboardEvent<HTMLDivElement>) {
        if (event.key === "ArrowLeft") {
            event.preventDefault()
            setSplitRatio(splitRatio - GIT_DIFF_SPLIT_STEP)
        } else if (event.key === "ArrowRight") {
            event.preventDefault()
            setSplitRatio(splitRatio + GIT_DIFF_SPLIT_STEP)
        } else if (event.key === "Home") {
            event.preventDefault()
            setSplitRatio(GIT_DIFF_SPLIT_MIN)
        } else if (event.key === "End") {
            event.preventDefault()
            setSplitRatio(GIT_DIFF_SPLIT_MAX)
        } else if (event.key === "Enter") {
            event.preventDefault()
            resetSplitRatio()
        }
    }

    if (reason) {
        return <EmptyState icon={FileWarning} title={t("diffView.unavailableTitle")} description={reason} />
    }

    const percent = Math.round(splitRatio * 100)
    const hasChunks = chunkState.total > 0
    const displayCurrent = hasChunks ? chunkState.current + 1 : 0

    // CodeMirror/MergeView owns scrolling via .cm-mergeView / .cm-scroller.
    // The host clips so app-owned scrollbars never nest around the editor viewport.
    return (
        <div className="flex h-full min-h-0 flex-col" data-testid="diff-view-root">
            <div className="flex h-[28px] shrink-0 items-center gap-[6px] border-b border-(--line-1) bg-(--paper-1) px-[10px]">
                <Button
                    type="button"
                    variant="ghost"
                    size="icon-xs"
                    disabled={!hasChunks}
                    aria-label={t("diffView.previousChange")}
                    aria-keyshortcuts="Shift+F7 Alt+ArrowUp"
                    onClick={() => stepChunk(-1)}
                    className="text-(--ink-2)"
                >
                    ‹
                </Button>
                <span
                    aria-live="polite"
                    className="min-w-0 flex-1 truncate text-center font-mono text-[11px] text-(--ink-2)"
                >
                    {hasChunks
                        ? t("diffView.chunkStatus", { current: displayCurrent, total: chunkState.total })
                        : t("diffView.noDifferences")}
                </span>
                <Button
                    type="button"
                    variant="ghost"
                    size="icon-xs"
                    disabled={!hasChunks}
                    aria-label={t("diffView.nextChange")}
                    aria-keyshortcuts="F7 Alt+ArrowDown"
                    onClick={() => stepChunk(1)}
                    className="text-(--ink-2)"
                >
                    ›
                </Button>
            </div>
            <div className="diff-view relative h-full min-h-0 flex-1 overflow-hidden" ref={containerRef}>
                {mode === "split" && (
                    <div
                        role="separator"
                        aria-orientation="vertical"
                        aria-valuemin={25}
                        aria-valuemax={75}
                        aria-valuenow={percent}
                        aria-label={t("diffView.splitSeparator")}
                        title={t("diffView.resetSplit")}
                        tabIndex={0}
                        data-testid="diff-split-separator"
                        className="absolute inset-y-0 z-10 w-[10px] -translate-x-1/2 cursor-col-resize touch-none outline-none focus-visible:ring-2 focus-visible:ring-(--yz-accent)"
                        style={{ left: `calc(var(--yz-diff-split-a) * 100%)` }}
                        onPointerDown={onSeparatorPointerDown}
                        onPointerMove={onSeparatorPointerMove}
                        onPointerUp={commitSeparator}
                        onPointerCancel={commitSeparator}
                        onDoubleClick={() => resetSplitRatio()}
                        onKeyDown={onSeparatorKeyDown}
                    >
                        <span className="pointer-events-none absolute inset-y-0 left-1/2 w-px -translate-x-1/2 bg-(--line-2)" />
                    </div>
                )}
            </div>
        </div>
    )
}

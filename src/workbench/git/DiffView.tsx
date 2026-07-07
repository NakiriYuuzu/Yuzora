import { useEffect, useRef } from "react"
import { EditorState } from "@codemirror/state"
import { EditorView, lineNumbers } from "@codemirror/view"
import { syntaxHighlighting } from "@codemirror/language"
import { MergeView, unifiedMergeView } from "@codemirror/merge"
import { FileWarning } from "lucide-react"
import { useTranslation } from "react-i18next"

import i18n from "@/lib/i18n"
import { appHighlightStyle, appTheme } from "@/editor/cmTheme"
import { hasVeryLongLine, languageExtensionFromPath } from "@/editor/cmExtensions"
import { EmptyState } from "@/app/workbench/EmptyState"
import type { DiffContent, GradedText } from "@/lib/types"

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

export function DiffView({ content, mode, path }: { content: DiffContent; mode: DiffMode; path: string }) {
    const { t } = useTranslation("menus")
    const containerRef = useRef<HTMLDivElement>(null)

    const reason = undisplayable(content.original) ?? undisplayable(content.modified)
    const original = docOf(content.original)
    const modified = docOf(content.modified)
    // Language facet lets @codemirror/merge highlight both sides (incl. deleted
    // lines via syntaxHighlightDeletions). Very long lines (minified diffs) skip
    // it to keep the parser from stalling, matching the editor's syntaxOff guard.
    const langExt =
        hasVeryLongLine(original) || hasVeryLongLine(modified) ? null : languageExtensionFromPath(path)

    useEffect(() => {
        if (reason) return
        const parent = containerRef.current
        if (!parent) return

        const langExtensions = langExt ? [langExt] : []
        let view: EditorView | MergeView
        if (mode === "split") {
            view = new MergeView({
                a: {
                    doc: original,
                    extensions: [
                        lineNumbers(),
                        ...themeExtensions,
                        ...langExtensions,
                        EditorState.readOnly.of(true)
                    ]
                },
                b: {
                    doc: modified,
                    extensions: [
                        lineNumbers(),
                        ...themeExtensions,
                        ...langExtensions,
                        EditorState.readOnly.of(true)
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
                    EditorState.readOnly.of(true)
                ],
                parent
            })
        }

        return () => view.destroy()
    }, [reason, original, modified, mode, path])

    if (reason) {
        return <EmptyState icon={FileWarning} title={t("diffView.unavailableTitle")} description={reason} />
    }
    return <div className="diff-view h-full min-h-0 overflow-auto" ref={containerRef} />
}

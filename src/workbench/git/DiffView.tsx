import { useEffect, useRef } from "react"
import { EditorState } from "@codemirror/state"
import { EditorView } from "@codemirror/view"
import { MergeView, unifiedMergeView } from "@codemirror/merge"
import { FileWarning } from "lucide-react"

import { EmptyState } from "@/app/workbench/EmptyState"
import type { DiffContent, GradedText } from "@/lib/types"

type DiffMode = "unified" | "split"

// tooLarge/binary carry no content, so they can't be diffed. Any such side
// short-circuits the whole view into a status message (spec: EmptyState,
// title「無法顯示 diff」, description by kind).
function undisplayable(side: GradedText): string | null {
    if (side.kind === "tooLarge") return "檔案過大（>50MB）"
    if (side.kind === "binary") return "二進位檔案"
    return null
}

// full/limited both carry content — limited is just a truncated slice from the
// Rust grading pipeline, so it diffs the same way.
function docOf(side: GradedText): string {
    return "content" in side ? side.content : ""
}

export function DiffView({ content, mode }: { content: DiffContent; mode: DiffMode }) {
    const containerRef = useRef<HTMLDivElement>(null)

    const reason = undisplayable(content.original) ?? undisplayable(content.modified)
    const original = docOf(content.original)
    const modified = docOf(content.modified)

    useEffect(() => {
        if (reason) return
        const parent = containerRef.current
        if (!parent) return

        let view: EditorView | MergeView
        if (mode === "split") {
            view = new MergeView({
                a: { doc: original, extensions: EditorState.readOnly.of(true) },
                b: { doc: modified, extensions: EditorState.readOnly.of(true) },
                parent
            })
        } else {
            view = new EditorView({
                doc: modified,
                extensions: [
                    unifiedMergeView({ original, mergeControls: false }),
                    EditorState.readOnly.of(true)
                ],
                parent
            })
        }

        return () => view.destroy()
    }, [reason, original, modified, mode])

    if (reason) {
        return <EmptyState icon={FileWarning} title="無法顯示 diff" description={reason} />
    }
    return <div className="diff-view h-full min-h-0 overflow-auto" ref={containerRef} />
}

import { EditorState, type Extension } from "@codemirror/state"
import {
    EditorView,
    keymap,
    lineNumbers,
    highlightActiveLine,
    highlightSpecialChars
} from "@codemirror/view"
import { defaultKeymap, history, historyKeymap, indentWithTab } from "@codemirror/commands"
import { searchKeymap, highlightSelectionMatches } from "@codemirror/search"
import { javascript } from "@codemirror/lang-javascript"
import { python } from "@codemirror/lang-python"
import { rust } from "@codemirror/lang-rust"
import { markdown } from "@codemirror/lang-markdown"
import { json } from "@codemirror/lang-json"
import { html } from "@codemirror/lang-html"
import { css } from "@codemirror/lang-css"
import { MAX_LINE_LEN_SYNTAX_OFF } from "../lib/types"

export function languageExtensionFromPath(path: string): Extension | null {
    const ext = path.split(".").pop()?.toLowerCase() ?? ""
    switch (ext) {
        case "ts":
        case "tsx":
            return javascript({ typescript: true, jsx: ext === "tsx" })
        case "js":
        case "jsx":
            return javascript({ jsx: ext === "jsx" })
        case "py":
            return python()
        case "rs":
            return rust()
        case "md":
            return markdown()
        case "json":
            return json()
        case "html":
            return html()
        case "css":
            return css()
        default:
            return null
    }
}

export function hasVeryLongLine(content: string): boolean {
    let start = 0
    while (start <= content.length) {
        const nl = content.indexOf("\n", start)
        const end = nl === -1 ? content.length : nl
        if (end - start > MAX_LINE_LEN_SYNTAX_OFF) return true
        if (nl === -1) break
        start = nl + 1
    }
    return false
}

export interface EditorFlags {
    readonly: boolean
    syntaxOff: boolean
}

export function buildExtensions(
    path: string,
    flags: EditorFlags,
    onDocChanged: () => void,
    onSave: () => void
): Extension[] {
    const extensions: Extension[] = [
        lineNumbers(),
        highlightSpecialChars(),
        highlightActiveLine(),
        highlightSelectionMatches(),
        history(),
        keymap.of([
            {
                key: "Mod-s",
                run: () => {
                    onSave()
                    return true
                }
            },
            ...defaultKeymap,
            ...historyKeymap,
            ...searchKeymap,
            indentWithTab
        ]),
        EditorView.updateListener.of((update) => {
            if (update.docChanged) onDocChanged()
        })
    ]
    if (!flags.syntaxOff) {
        const lang = languageExtensionFromPath(path)
        if (lang) extensions.push(lang)
    }
    if (flags.readonly) {
        extensions.push(EditorState.readOnly.of(true), EditorView.editable.of(false))
    }
    return extensions
}

import { EditorState, type Extension } from "@codemirror/state"
import {
    EditorView,
    keymap,
    lineNumbers,
    highlightActiveLine,
    highlightSpecialChars
} from "@codemirror/view"
import { defaultHighlightStyle, StreamLanguage, syntaxHighlighting } from "@codemirror/language"
import { defaultKeymap, history, historyKeymap, indentWithTab } from "@codemirror/commands"
import { searchKeymap, highlightSelectionMatches } from "@codemirror/search"
import { javascript } from "@codemirror/lang-javascript"
import { python } from "@codemirror/lang-python"
import { rust } from "@codemirror/lang-rust"
import { markdown } from "@codemirror/lang-markdown"
import { json } from "@codemirror/lang-json"
import { html } from "@codemirror/lang-html"
import { css } from "@codemirror/lang-css"
import { yaml } from "@codemirror/lang-yaml"
import { sql } from "@codemirror/lang-sql"
import { xml } from "@codemirror/lang-xml"
import { cpp } from "@codemirror/lang-cpp"
import { java } from "@codemirror/lang-java"
import { go } from "@codemirror/lang-go"
import { php } from "@codemirror/lang-php"
import { sass } from "@codemirror/lang-sass"
import { less } from "@codemirror/lang-less"
import { vue } from "@codemirror/lang-vue"
import { shell } from "@codemirror/legacy-modes/mode/shell"
import { toml } from "@codemirror/legacy-modes/mode/toml"
import { ruby } from "@codemirror/legacy-modes/mode/ruby"
import { swift } from "@codemirror/legacy-modes/mode/swift"
import { lua } from "@codemirror/legacy-modes/mode/lua"
import { perl } from "@codemirror/legacy-modes/mode/perl"
import { r } from "@codemirror/legacy-modes/mode/r"
import { dockerFile } from "@codemirror/legacy-modes/mode/dockerfile"
import { diff } from "@codemirror/legacy-modes/mode/diff"
import { properties } from "@codemirror/legacy-modes/mode/properties"
import { appHighlightStyle, appTheme } from "./cmTheme"
import { minimap, minimapCompartment } from "./minimap"
import { MAX_LINE_LEN_SYNTAX_OFF } from "../lib/types"

export function languageExtensionFromPath(path: string): Extension | null {
    const basename = path.split(/[\\/]/).pop() ?? path
    if (basename.toLowerCase() === "dockerfile") return StreamLanguage.define(dockerFile)
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
        case "yml":
        case "yaml":
            return yaml()
        case "sql":
            return sql()
        case "xml":
            return xml()
        case "c":
        case "h":
        case "cc":
        case "cpp":
        case "cxx":
        case "hpp":
            return cpp()
        case "java":
            return java()
        case "go":
            return go()
        case "php":
            return php()
        case "scss":
            return sass()
        case "sass":
            return sass({ indented: true })
        case "less":
            return less()
        case "vue":
            return vue()
        case "sh":
        case "bash":
        case "zsh":
            return StreamLanguage.define(shell)
        case "toml":
            return StreamLanguage.define(toml)
        case "rb":
            return StreamLanguage.define(ruby)
        case "swift":
            return StreamLanguage.define(swift)
        case "lua":
            return StreamLanguage.define(lua)
        case "pl":
            return StreamLanguage.define(perl)
        case "r":
            return StreamLanguage.define(r)
        case "dockerfile":
            return StreamLanguage.define(dockerFile)
        case "diff":
        case "patch":
            return StreamLanguage.define(diff)
        case "properties":
        case "ini":
            return StreamLanguage.define(properties)
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
    onSave: () => void,
    minimapEnabled: boolean
): Extension[] {
    const extensions: Extension[] = [
        appTheme,
        lineNumbers(),
        highlightSpecialChars(),
        highlightActiveLine(),
        highlightSelectionMatches(),
        syntaxHighlighting(appHighlightStyle),
        syntaxHighlighting(defaultHighlightStyle, { fallback: true }),
        minimapCompartment.of(minimap(minimapEnabled)),
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

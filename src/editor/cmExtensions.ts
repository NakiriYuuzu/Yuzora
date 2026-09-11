import { EditorState, type Extension } from "@codemirror/state"
import {
    EditorView,
    keymap,
    lineNumbers,
    highlightActiveLine,
    highlightSpecialChars
} from "@codemirror/view"
import { defaultHighlightStyle, LanguageSupport, LRLanguage, StreamLanguage, syntaxHighlighting } from "@codemirror/language"
import { styleTags, tags, type Tag } from "@lezer/highlight"
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
import { svelte } from "@replit/codemirror-lang-svelte"
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
import { csharp, kotlin, dart, scala } from "@codemirror/legacy-modes/mode/clike"
import { powerShell } from "@codemirror/legacy-modes/mode/powershell"
import { appHighlightStyle, appTheme } from "./cmTheme"
import { minimap, minimapCompartment } from "./minimap"
import { MAX_LINE_LEN_SYNTAX_OFF } from "../lib/types"

// Add only context-specific roles that the bundled grammars leave generic.
// Preserve each language's parser, completion/indentation support and dialect.
function withSyntaxTags(support: LanguageSupport, styles: Record<string, Tag>): LanguageSupport {
    return new LanguageSupport((support.language as LRLanguage).configure({ props: [styleTags(styles)] }), support.support)
}
function stylesheet(support: LanguageSupport): LanguageSupport {
    return withSyntaxTags(support, { Callee: tags.function(tags.variableName) })
}

export function languageExtensionFromPath(path: string): Extension | null {
    const basename = path.split(/[\\/]/).pop() ?? path
    const name = basename.toLowerCase()
    if (name === "dockerfile" || name === "containerfile" || name.startsWith("dockerfile.")) return StreamLanguage.define(dockerFile)
    if ([".bashrc", ".bash_profile", ".zshrc", ".zprofile", ".profile"].includes(name) || name === ".env" || name.startsWith(".env.")) return StreamLanguage.define(shell)
    if ([".gitconfig", ".editorconfig", ".npmrc", ".yarnrc"].includes(name)) return StreamLanguage.define(properties)
    const ext = basename.includes(".") ? name.split(".").pop() ?? "" : ""
    switch (ext) {
        case "mts":
        case "cts":
        case "ts":
        case "tsx":
            return javascript({ typescript: true, jsx: ext === "tsx" })
        case "mjs":
        case "cjs":
        case "js":
        case "jsx":
            return javascript({ jsx: ext === "jsx" })
        case "pyi":
        case "pyw":
        case "py":
            return python()
        case "rs":
            return rust()
        case "markdown":
        case "md":
            return markdown()
        case "json":
            return json()
        case "htm":
        case "html":
            return html()
        case "css":
            return stylesheet(css())
        case "yml":
        case "yaml":
            return yaml()
        case "sql":
            return sql()
        case "svg":
        case "xsd":
        case "xsl":
        case "csproj":
        case "fsproj":
        case "xml":
            return xml()
        case "c":
        case "h":
        case "cc":
        case "cpp":
        case "cxx":
        case "hh":
        case "hxx":
        case "hpp":
            return withSyntaxTags(cpp(), { "FunctionDeclarator/FieldIdentifier": tags.function(tags.definition(tags.propertyName)) })
        case "java":
            return java()
        case "go":
            return go()
        case "php":
            return php()
        case "scss":
            return stylesheet(sass())
        case "sass":
            return stylesheet(sass({ indented: true }))
        case "less":
            return stylesheet(less())
        case "vue":
            return vue({ base: html({ nestedLanguages: [
                { tag: "script", attrs: attrs => attrs.lang === "tsx", parser: javascript({ typescript: true, jsx: true }).language.parser },
                { tag: "script", attrs: attrs => attrs.lang === "jsx", parser: javascript({ jsx: true }).language.parser },
                { tag: "script", attrs: attrs => attrs.lang === "typescript", parser: javascript({ typescript: true }).language.parser },
                { tag: "style", attrs: attrs => attrs.lang === "scss", parser: stylesheet(sass()).language.parser },
                { tag: "style", attrs: attrs => attrs.lang === "sass", parser: stylesheet(sass({ indented: true })).language.parser },
                { tag: "style", attrs: attrs => attrs.lang === "less", parser: stylesheet(less()).language.parser },
            ] }) })
        case "svelte":
            return svelte()
        case "sh":
        case "bash":
        case "zsh":
            return StreamLanguage.define(shell)
        case "toml":
            return StreamLanguage.define(toml)
        case "rb":
            return StreamLanguage.define(ruby)
        case "cs":
        case "csx":
            return StreamLanguage.define(csharp)
        case "kt":
        case "kts":
            return StreamLanguage.define(kotlin)
        case "dart":
            return StreamLanguage.define(dart)
        case "scala":
        case "sc":
            return StreamLanguage.define(scala)
        case "ps1":
        case "psm1":
        case "psd1":
            return StreamLanguage.define(powerShell)
        case "swift":
            return StreamLanguage.define(swift)
        case "lua":
            return StreamLanguage.define(lua)
        case "pm":
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

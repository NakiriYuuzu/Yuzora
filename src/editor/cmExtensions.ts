import { Compartment, EditorState, type Extension } from "@codemirror/state"
import {
    EditorView,
    ViewPlugin,
    keymap,
    lineNumbers,
    highlightActiveLine,
    highlightSpecialChars
} from "@codemirror/view"
import { defaultHighlightStyle, LanguageSupport, LRLanguage, StreamLanguage, syntaxHighlighting } from "@codemirror/language"
import { styleTags, tags, type Tag } from "@lezer/highlight"
import { defaultKeymap, history, historyKeymap, indentWithTab } from "@codemirror/commands"
import { searchKeymap, highlightSelectionMatches } from "@codemirror/search"
import { appHighlightStyle, appTheme } from "./cmTheme"
import { minimap, minimapCompartment } from "./minimap"
import { largeFileSearch } from "./largeFileSearch"
import { MAX_LINE_LEN_SYNTAX_OFF } from "../lib/types"

// Add only context-specific roles that the bundled grammars leave generic.
// Preserve each language's parser, completion/indentation support and dialect.
function withSyntaxTags(support: LanguageSupport, styles: Record<string, Tag>): LanguageSupport {
    return new LanguageSupport((support.language as LRLanguage).configure({ props: [styleTags(styles)] }), support.support)
}
function stylesheet(support: LanguageSupport): LanguageSupport {
    // Context specificity takes precedence over the grammar's generic Callee tag.
    return withSyntaxTags(support, { "CallExpression/Callee": tags.function(tags.variableName) })
}

function languageKey(path: string): string {
    const basename = path.split(/[\\/]/).pop() ?? path
    const name = basename.toLowerCase()
    if (name === "dockerfile" || name === "containerfile" || name.startsWith("dockerfile.")) return "dockerfile"
    if ([".bashrc", ".bash_profile", ".zshrc", ".zprofile", ".profile"].includes(name) || name === ".env" || name.startsWith(".env.")) return "sh"
    if ([".gitconfig", ".editorconfig", ".npmrc", ".yarnrc"].includes(name)) return "properties"
    const ext = basename.includes(".") ? name.split(".").pop() ?? "" : ""
    return ext
}

async function importLanguage(ext: string): Promise<Extension | null> {
    switch (ext) {
        case "mts":
        case "cts":
        case "ts":
        case "tsx":
            return (await import("@codemirror/lang-javascript")).javascript({ typescript: true, jsx: ext === "tsx" })
        case "mjs":
        case "cjs":
        case "js":
        case "jsx":
            return (await import("@codemirror/lang-javascript")).javascript({ jsx: ext === "jsx" })
        case "pyi":
        case "pyw":
        case "py":
            return (await import("@codemirror/lang-python")).python()
        case "rs":
            return (await import("@codemirror/lang-rust")).rust()
        case "markdown":
        case "md":
            return (await import("@codemirror/lang-markdown")).markdown()
        case "json":
            return (await import("@codemirror/lang-json")).json()
        case "htm":
        case "html":
            return (await import("@codemirror/lang-html")).html()
        case "css":
            return stylesheet((await import("@codemirror/lang-css")).css())
        case "yml":
        case "yaml":
            return (await import("@codemirror/lang-yaml")).yaml()
        case "sql":
            return (await import("@codemirror/lang-sql")).sql()
        case "svg":
        case "xsd":
        case "xsl":
        case "csproj":
        case "fsproj":
        case "xml":
            return (await import("@codemirror/lang-xml")).xml()
        case "c":
        case "h":
        case "cc":
        case "cpp":
        case "cxx":
        case "hh":
        case "hxx":
        case "hpp":
            return withSyntaxTags((await import("@codemirror/lang-cpp")).cpp(), { "FunctionDeclarator/FieldIdentifier": tags.function(tags.definition(tags.propertyName)) })
        case "java":
            return (await import("@codemirror/lang-java")).java()
        case "go":
            return (await import("@codemirror/lang-go")).go()
        case "php":
            return (await import("@codemirror/lang-php")).php()
        case "scss":
            return stylesheet((await import("@codemirror/lang-sass")).sass())
        case "sass":
            return stylesheet((await import("@codemirror/lang-sass")).sass({ indented: true }))
        case "less":
            return stylesheet((await import("@codemirror/lang-less")).less())
        case "vue": {
            const [{ vue }, { html }, { javascript }, { sass }, { less }] = await Promise.all([
                import("@codemirror/lang-vue"),
                import("@codemirror/lang-html"),
                import("@codemirror/lang-javascript"),
                import("@codemirror/lang-sass"),
                import("@codemirror/lang-less"),
            ])
            return vue({ base: html({ nestedLanguages: [
                { tag: "script", attrs: attrs => attrs.lang === "tsx", parser: javascript({ typescript: true, jsx: true }).language.parser },
                { tag: "script", attrs: attrs => attrs.lang === "jsx", parser: javascript({ jsx: true }).language.parser },
                { tag: "script", attrs: attrs => attrs.lang === "typescript", parser: javascript({ typescript: true }).language.parser },
                { tag: "style", attrs: attrs => attrs.lang === "scss", parser: stylesheet(sass()).language.parser },
                { tag: "style", attrs: attrs => attrs.lang === "sass", parser: stylesheet(sass({ indented: true })).language.parser },
                { tag: "style", attrs: attrs => attrs.lang === "less", parser: stylesheet(less()).language.parser },
            ] }) })
        }
        case "svelte":
            return (await import("@replit/codemirror-lang-svelte")).svelte()
        case "sh":
        case "bash":
        case "zsh":
            return StreamLanguage.define((await import("@codemirror/legacy-modes/mode/shell")).shell)
        case "toml":
            return StreamLanguage.define((await import("@codemirror/legacy-modes/mode/toml")).toml)
        case "rb":
            return StreamLanguage.define((await import("@codemirror/legacy-modes/mode/ruby")).ruby)
        case "cs":
        case "csx":
            return StreamLanguage.define((await import("@codemirror/legacy-modes/mode/clike")).csharp)
        case "kt":
        case "kts":
            return StreamLanguage.define((await import("@codemirror/legacy-modes/mode/clike")).kotlin)
        case "dart":
            return StreamLanguage.define((await import("@codemirror/legacy-modes/mode/clike")).dart)
        case "scala":
        case "sc":
            return StreamLanguage.define((await import("@codemirror/legacy-modes/mode/clike")).scala)
        case "ps1":
        case "psm1":
        case "psd1":
            return StreamLanguage.define((await import("@codemirror/legacy-modes/mode/powershell")).powerShell)
        case "swift":
            return StreamLanguage.define((await import("@codemirror/legacy-modes/mode/swift")).swift)
        case "lua":
            return StreamLanguage.define((await import("@codemirror/legacy-modes/mode/lua")).lua)
        case "pm":
        case "pl":
            return StreamLanguage.define((await import("@codemirror/legacy-modes/mode/perl")).perl)
        case "r":
            return StreamLanguage.define((await import("@codemirror/legacy-modes/mode/r")).r)
        case "dockerfile":
            return StreamLanguage.define((await import("@codemirror/legacy-modes/mode/dockerfile")).dockerFile)
        case "diff":
        case "patch":
            return StreamLanguage.define((await import("@codemirror/legacy-modes/mode/diff")).diff)
        case "properties":
        case "ini":
            return StreamLanguage.define((await import("@codemirror/legacy-modes/mode/properties")).properties)
        default:
            return null
    }
}

// Cache by language/dialect, not document path; concurrent panes share one load.
const languages = new Map<string, Extension | null>()
const pendingLanguages = new Map<string, Promise<Extension | null>>()

export function languageExtensionFromPath(path: string): Extension | null {
    return languages.get(languageKey(path)) ?? null
}

export function loadLanguageExtension(path: string): Promise<Extension | null> {
    const key = languageKey(path)
    if (languages.has(key)) return Promise.resolve(languages.get(key) ?? null)
    const pending = pendingLanguages.get(key)
    if (pending) return pending
    const load = importLanguage(key).then(extension => {
        languages.set(key, extension)
        pendingLanguages.delete(key)
        return extension
    }, error => {
        pendingLanguages.delete(key)
        throw error
    })
    pendingLanguages.set(key, load)
    return load
}

// The plugin belongs to the view, so a late grammar never touches a destroyed
// pane. Reconfiguration changes only syntax, preserving document/history/scroll.
export function languageExtensions(path: string, syntaxOff: boolean, onLoaded?: (view: EditorView) => void): Extension[] {
    if (syntaxOff) return []
    const compartment = new Compartment()
    const initialExtension = languageExtensionFromPath(path)
    return [
        compartment.of(initialExtension ?? []),
        ViewPlugin.define(view => {
            let disposed = false
            void loadLanguageExtension(path).then(extension => {
                if (!disposed && extension) {
                    if (extension !== initialExtension) {
                        view.dispatch({ effects: compartment.reconfigure(extension) })
                    }
                    onLoaded?.(view)
                }
            }).catch(() => {
                // A failed optional grammar must not prevent editing or saving.
                if (!disposed) console.warn("Syntax grammar could not be loaded")
            })
            return { destroy() { disposed = true } }
        }),
    ]
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
        largeFileSearch(),
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
    extensions.push(...languageExtensions(path, flags.syntaxOff))
    if (flags.readonly) {
        extensions.push(EditorState.readOnly.of(true), EditorView.editable.of(false))
    }
    return extensions
}

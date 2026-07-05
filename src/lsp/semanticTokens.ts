import { RangeSetBuilder, StateEffect, StateField, type Extension, type Text } from "@codemirror/state"
import { Decoration, type DecorationSet, EditorView, ViewPlugin, type ViewUpdate } from "@codemirror/view"
import type { LSPClient } from "@codemirror/lsp-client"

// The server-declared legend maps the numeric tokenType / tokenModifier indices
// in the delta stream back to names. Defined locally (not in lib/types) per T8 scope.
export interface SemanticTokensLegend {
    tokenTypes: string[]
    tokenModifiers: string[]
}

// One decoded token: absolute (line, char) position, its length, its resolved
// type name, and the list of resolved modifier names.
export interface DecodedToken {
    line: number
    char: number
    length: number
    type: string
    modifiers: string[]
}

// The 22 standard LSP semantic token types (SemanticTokenTypes) plus `decorator`.
// A token whose resolved name is outside this set gets no decoration (safe ignore)
// — its CSS class isn't in editor.css and, per plan, non-standard names must not
// throw. Used only for the render-time filter; decode itself keeps every named token.
const STANDARD_TOKEN_TYPES = new Set([
    "namespace", "type", "class", "enum", "interface", "struct", "typeParameter",
    "parameter", "variable", "property", "enumMember", "event", "function", "method",
    "macro", "keyword", "modifier", "comment", "string", "number", "regexp", "operator",
    "decorator"
])

// The 10 standard LSP semantic token modifiers (SemanticTokenModifiers).
const STANDARD_MODIFIERS = new Set([
    "declaration", "definition", "readonly", "static", "deprecated", "abstract",
    "async", "modification", "documentation", "defaultLibrary"
])

// Decode the LSP full semantic-tokens delta stream (groups of 5 uintegers:
// [deltaLine, deltaStartChar, length, tokenType, tokenModifiers]) into absolute
// tokens. deltaLine/deltaStartChar accumulate across tokens: same line -> char
// adds to the running char; a line advance resets char to deltaStartChar. Indices
// that fall outside the legend are safely ignored (the token is dropped; a modifier
// bit with no legend entry is skipped) rather than throwing.
export function decodeSemanticTokens(data: number[], legend: SemanticTokensLegend): DecodedToken[] {
    const tokens: DecodedToken[] = []
    let line = 0
    let char = 0

    for (let i = 0; i + 4 < data.length; i += 5) {
        const deltaLine = data[i]
        const deltaStartChar = data[i + 1]
        const length = data[i + 2]
        const tokenType = data[i + 3]
        const tokenModifiers = data[i + 4]

        if (deltaLine === 0) {
            char += deltaStartChar
        } else {
            line += deltaLine
            char = deltaStartChar
        }

        // Position is advanced before the range check so a dropped token still
        // keeps the running (line, char) correct for the tokens that follow.
        const type = legend.tokenTypes[tokenType]
        if (type === undefined) continue

        const modifiers: string[] = []
        for (let b = 0; b < legend.tokenModifiers.length; b++) {
            if (tokenModifiers & (1 << b)) modifiers.push(legend.tokenModifiers[b])
        }

        tokens.push({ line, char, length, type, modifiers })
    }

    return tokens
}

// Map one decoded token to its space-separated class list, or null when the type
// name is non-standard (no editor.css rule exists — safe ignore per plan). The
// base class is `cm-st-<type>`; each standard modifier adds `cm-st-mod-<modifier>`.
function classesFor(token: DecodedToken): string | null {
    if (!STANDARD_TOKEN_TYPES.has(token.type)) return null
    let cls = `cm-st-${token.type}`
    for (const m of token.modifiers) {
        if (STANDARD_MODIFIERS.has(m)) cls += ` cm-st-mod-${m}`
    }
    return cls
}

// Build a DecorationSet of mark decorations from decoded tokens against `doc`.
// Out-of-range lines, empty spans, non-standard types, and out-of-order tokens
// (a `from` that would move backwards — RangeSetBuilder requires a non-decreasing
// `from`) are skipped so a stale or malformed server payload can never throw
// during a view update.
function buildDecorations(tokens: DecodedToken[], doc: Text): DecorationSet {
    const builder = new RangeSetBuilder<Decoration>()
    let lastFrom = -1
    for (const token of tokens) {
        if (token.length <= 0) continue
        if (token.line < 0 || token.line >= doc.lines) continue
        const cls = classesFor(token)
        if (!cls) continue

        const lineObj = doc.line(token.line + 1)
        const from = lineObj.from + token.char
        if (from < lineObj.from || from > lineObj.to) continue
        const to = Math.min(from + token.length, lineObj.to)
        if (from >= to) continue
        // A negative delta can push `from` behind the previous token; RangeSetBuilder
        // requires additions in non-decreasing `from` order, so drop the offender.
        if (from < lastFrom) continue

        builder.add(from, to, Decoration.mark({ class: cls }))
        lastFrom = from
    }
    return builder.finish()
}

// Effect carrying a freshly decoded token set into the decoration field. The
// request is async, so decorations arrive via this effect rather than being
// derived synchronously from the document.
const setSemanticTokens = StateEffect.define<DecodedToken[]>()

// Holds the current semantic-token decorations. On document changes the existing
// set is remapped through the change so highlights track edits until the next
// fetch replaces them; a setSemanticTokens effect rebuilds against the new doc.
const semanticTokenField = StateField.define<DecorationSet>({
    create: () => Decoration.none,
    update(deco, tr) {
        deco = deco.map(tr.changes)
        for (const e of tr.effects) {
            if (e.is(setSemanticTokens)) deco = buildDecorations(e.value, tr.state.doc)
        }
        return deco
    },
    provide: (f) => EditorView.decorations.from(f)
})

// Debounce for re-fetching after edits — matches the LSP push/pull cadence used
// elsewhere in the manager (~500ms). The initial load is immediate.
const DEBOUNCE_MS = 500

// Minimal structural view of the request/response we consume, kept local so this
// file doesn't depend on the undeclared vscode-languageserver-protocol types (the
// same convention lspManager.ts uses for ServerCapabilities).
interface SemanticTokensResult {
    data?: number[]
}

function fetchPlugin(client: LSPClient, uri: string, legend: SemanticTokensLegend) {
    return ViewPlugin.fromClass(
        class {
            private timer: ReturnType<typeof setTimeout> | null = null
            // Bumped on every document change so an in-flight fetch started against
            // an older document is discarded instead of overwriting fresh tokens.
            private generation = 0

            constructor(private readonly view: EditorView) {
                // Initial load is immediate; edits are debounced.
                this.fetch()
            }

            update(u: ViewUpdate) {
                if (u.docChanged) {
                    this.generation++
                    this.schedule()
                }
            }

            private schedule() {
                if (this.timer !== null) clearTimeout(this.timer)
                this.timer = setTimeout(() => {
                    this.timer = null
                    this.fetch()
                }, DEBOUNCE_MS)
            }

            private fetch() {
                const gen = this.generation
                void client
                    .request<{ textDocument: { uri: string } }, SemanticTokensResult | null>(
                        "textDocument/semanticTokens/full",
                        { textDocument: { uri } }
                    )
                    .then((result) => {
                        // The document moved on while this request was in flight — a
                        // newer fetch is already scheduled, so drop this stale result.
                        if (gen !== this.generation) return
                        if (!result || !result.data) return
                        const tokens = decodeSemanticTokens(result.data, legend)
                        this.view.dispatch({ effects: setSemanticTokens.of(tokens) })
                    })
                    .catch(() => {
                        // Request failed / cancelled / timed out — keep the last
                        // decorations (or none). Lezer baseline highlighting stays.
                    })
            }

            destroy() {
                if (this.timer !== null) clearTimeout(this.timer)
            }
        }
    )
}

// Semantic-token highlighting layered over the Lezer baseline. Returns an empty
// extension (graceful degradation — baseline preserved) when the server does not
// advertise semanticTokensProvider or omits its legend. The legend is declared by
// the server; non-standard names in it are ignored at render time, never thrown.
export function semanticTokensExtension(client: LSPClient, uri: string): Extension {
    const provider = client.serverCapabilities?.semanticTokensProvider
    if (!provider || typeof provider !== "object" || !provider.legend) return []

    const legend: SemanticTokensLegend = {
        tokenTypes: provider.legend.tokenTypes ?? [],
        tokenModifiers: provider.legend.tokenModifiers ?? []
    }

    return [semanticTokenField, fetchPlugin(client, uri, legend)]
}

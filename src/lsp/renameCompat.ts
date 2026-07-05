// Minimal, self-contained mirror of the LSP protocol types this compat layer
// touches. The full definitions live in vscode-languageserver-protocol — a
// transitive dependency we do not declare in package.json (same stance as
// lspManager's ServerCapabilities mirror) — so we spell out only the fields the
// rename / code-action edit mapping reads. Exported for codeActions.ts to share.
export interface Position {
    line: number
    character: number
}
export interface Range {
    start: Position
    end: Position
}
export interface TextEdit {
    range: Range
    newText: string
}
// documentChanges entries carry a (possibly null) document version, unlike the
// bare uri->edits map in `changes`.
export interface OptionalVersionedTextDocumentIdentifier {
    uri: string
    version: number | null
}
export interface TextDocumentEdit {
    textDocument: OptionalVersionedTextDocumentIdentifier
    edits: TextEdit[]
}
export interface WorkspaceEdit {
    changes?: { [uri: string]: TextEdit[] }
    documentChanges?: TextDocumentEdit[]
}

// Normalizes a WorkspaceEdit's two on-the-wire shapes into one uri/edits list.
//
// rust-analyzer returns `documentChanges` (versioned TextDocumentEdit[]); vtsls
// returns `changes` ({[uri]: TextEdit[]}). The official rename.ts only reads
// `changes`, so a rust-analyzer rename is applied to nothing and silently fails
// (T9 #4 root cause). Per LSP spec, when documentChanges is present it is
// authoritative and `changes` must be ignored.
export function workspaceEditToChanges(edit: WorkspaceEdit): Array<{ uri: string; edits: TextEdit[] }> {
    if (edit.documentChanges && edit.documentChanges.length > 0) {
        // A documentChanges array may also carry resource operations (create /
        // rename / delete file), which have no `edits`. Keep only text edits —
        // resource ops are out of this layer's scope.
        return edit.documentChanges
            .filter((dc) => dc != null && dc.textDocument != null && Array.isArray(dc.edits))
            .map((dc) => ({ uri: dc.textDocument.uri, edits: dc.edits }))
    }
    if (edit.changes) {
        return Object.entries(edit.changes).map(([uri, edits]) => ({ uri, edits }))
    }
    return []
}

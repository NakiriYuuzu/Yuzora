import type { LSPClient } from "@codemirror/lsp-client"

import type { Position, Range } from "./renameCompat"

// Flattened document-outline entry. Both LSP response shapes (hierarchical
// DocumentSymbol[] and flat SymbolInformation[]) map onto this. `range` is the
// location to jump to when the symbol is selected; `detail` (e.g. a signature)
// is only present when the server supplies it.
export interface FlatSymbol {
    name: string
    kind: number
    range: Range
    detail?: string
}

// A workspace/symbol hit: unlike document symbols it carries the owning file's
// uri so the picker can open a (possibly unopened) file before revealing.
export interface WorkspaceSymbolItem {
    name: string
    kind: number
    uri: string
    range: Range
}

const ZERO_POSITION: Position = { line: 0, character: 0 }
const ZERO_RANGE: Range = { start: ZERO_POSITION, end: ZERO_POSITION }

// A structurally-valid Range: a non-conforming server may send e.g. `range: {}`
// (truthy but without .start), which a bare nullish / truthy guard lets through —
// selecting the symbol then dereferences `range.start.line` and throws. Require
// the shape the picker actually reads before trusting it; otherwise ZERO_RANGE.
function isValidRange(r: unknown): r is Range {
    const start = (r as { start?: Partial<Position> } | null | undefined)?.start
    return typeof start?.line === "number" && typeof start?.character === "number"
}

// A hierarchical DocumentSymbol: has range + selectionRange and may nest.
interface RawDocumentSymbol {
    name: string
    kind: number
    detail?: string
    range: Range
    selectionRange?: Range
    children?: RawDocumentSymbol[]
}

// A flat SymbolInformation: carries a location instead of range/children.
interface RawSymbolInformation {
    name: string
    kind: number
    location: { uri: string; range: Range }
}

// Depth-first pre-order flatten of a DocumentSymbol subtree (parent before its
// children) so the list reads top-to-bottom like the source outline. The flat
// `range` uses selectionRange (the identifier's location) when present, so a
// jump lands on the name rather than the whole body, falling back to range.
function flattenSymbol(node: unknown, out: FlatSymbol[]): void {
    if (node == null || typeof node !== "object") return
    const rec = node as Record<string, unknown>

    // SymbolInformation (flat): detected by `location`.
    if (rec.location != null && typeof rec.location === "object") {
        const info = node as RawSymbolInformation
        const range = isValidRange(info.location.range) ? info.location.range : ZERO_RANGE
        out.push({ name: info.name, kind: info.kind, range })
        return
    }

    // DocumentSymbol (hierarchical): detected by selectionRange / children. Prefer
    // selectionRange (the name location) so a jump lands on the identifier, then
    // range, degrading to ZERO_RANGE if neither is a well-formed range.
    const sym = node as RawDocumentSymbol
    const range = isValidRange(sym.selectionRange)
        ? sym.selectionRange
        : isValidRange(sym.range)
          ? sym.range
          : ZERO_RANGE
    const flat: FlatSymbol = { name: sym.name, kind: sym.kind, range }
    if (sym.detail != null) flat.detail = sym.detail
    out.push(flat)
    if (Array.isArray(sym.children)) {
        for (const child of sym.children) flattenSymbol(child, out)
    }
}

// textDocument/documentSymbol → a flat outline. The server may answer with
// either DocumentSymbol[] (hierarchical) or SymbolInformation[] (flat); both are
// normalized here. A null / non-array response (no provider, empty document)
// yields [] rather than throwing — capability gating is the caller's job.
export async function requestDocumentSymbols(client: LSPClient, uri: string): Promise<FlatSymbol[]> {
    // Flush pending (debounced) didChange first so the server answers against the
    // buffer the user sees, not a stale sync point (as the built-in features do).
    client.sync()
    const result = await client.request<{ textDocument: { uri: string } }, unknown>(
        "textDocument/documentSymbol",
        { textDocument: { uri } }
    )
    if (!Array.isArray(result)) return []
    const out: FlatSymbol[] = []
    for (const item of result) flattenSymbol(item, out)
    return out
}

// workspace/symbol → cross-file symbol hits for a query. Handles both the
// legacy SymbolInformation (location.uri/location.range) and the newer
// WorkspaceSymbol whose location may carry only a uri (range resolved lazily) —
// a missing range degrades to a zero range so selection still opens the file. A
// null / non-array response yields [] without throwing.
export async function requestWorkspaceSymbols(
    client: LSPClient,
    query: string
): Promise<WorkspaceSymbolItem[]> {
    // Flush pending edits before the query, same as requestDocumentSymbols.
    client.sync()
    const result = await client.request<{ query: string }, unknown>("workspace/symbol", { query })
    if (!Array.isArray(result)) return []
    const out: WorkspaceSymbolItem[] = []
    for (const item of result) {
        if (item == null || typeof item !== "object") continue
        const rec = item as Record<string, unknown>
        const location = rec.location as { uri?: string; range?: Range } | undefined
        if (!location || typeof location.uri !== "string") continue
        // The picker renders item.name; drop a nameless record rather than
        // surfacing `undefined` (kind is display-only, so it stays best-effort).
        if (typeof rec.name !== "string") continue
        out.push({
            name: rec.name as string,
            kind: rec.kind as number,
            uri: location.uri,
            range: isValidRange(location.range) ? location.range : ZERO_RANGE
        })
    }
    return out
}

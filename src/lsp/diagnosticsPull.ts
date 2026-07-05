// Pull-mode diagnostics for LSP servers that answer textDocument/diagnostic but
// never push (rust-analyzer). Its building blocks — offsetOf, toLintDiagnostic,
// pullDiagnostics — are the shared, tested foundation the quick-fix linter
// (codeActions.lspLintSource) is built on and are imported across the LSP layer.
//
// diagnosticsPullExtension itself is a tested reference implementation that is
// intentionally NOT wired into assembleLspExtensions: the linter path
// (lspLintSource) is the superset — same pull loop PLUS quick-fix code actions —
// so wiring both would render every diagnostic twice. This standalone extension is
// kept for a future à-la-carte wiring and as the plain-pull reference to compare
// the linter path against.
import { ViewPlugin } from "@codemirror/view"
import type { EditorView, ViewUpdate } from "@codemirror/view"
import type { Extension, Text } from "@codemirror/state"
import { setDiagnostics } from "@codemirror/lint"
import type { Diagnostic } from "@codemirror/lint"
import type { LSPClient } from "@codemirror/lsp-client"

import type { Position, Range } from "./renameCompat"

// Structural mirror of an LSP Diagnostic (only the fields we render / forward to
// code actions). Range comes from renameCompat so the whole LSP position vocab
// stays defined in one place.
export interface LspDiagnostic {
    range: Range
    message: string
    severity?: number
    code?: string | number
    source?: string
}

// LSP DocumentDiagnosticReport: a "full" report carries `items`; an "unchanged"
// report just references a prior resultId and means "no change since last pull".
interface DocumentDiagnosticReport {
    kind?: "full" | "unchanged"
    items?: LspDiagnostic[]
}

// Match the push path's debounce (serverDiagnostics' autoSync) so pull-mode
// servers get diagnostics refreshed at the same cadence as push-mode ones.
const DEBOUNCE_MS = 500

// Convert an LSP {line, character} to a CodeMirror document offset. Kept local
// (rather than going through LSPPlugin.fromPosition) so this and codeActions can
// map positions against a bare view/doc without a live LSP plugin. Clamped so a
// stale position from a just-edited document can never throw.
export function offsetOf(doc: Text, pos: Position): number {
    const lineNo = Math.min(Math.max(pos.line, 0) + 1, doc.lines)
    const line = doc.line(lineNo)
    return Math.min(line.from + Math.max(pos.character, 0), line.to)
}

// Same 1..4 -> severity mapping the official serverDiagnostics uses.
function toSeverity(sev: number): Diagnostic["severity"] {
    return sev === 1 ? "error" : sev === 2 ? "warning" : sev === 3 ? "info" : "hint"
}

// Convert an LSP diagnostic into a CodeMirror lint Diagnostic (no actions). The
// pull loop and T7's lspLintSource both start from this; lspLintSource layers
// quick-fix actions on top.
export function toLintDiagnostic(doc: Text, item: LspDiagnostic): Diagnostic {
    return {
        from: offsetOf(doc, item.range.start),
        to: offsetOf(doc, item.range.end),
        severity: toSeverity(item.severity ?? 1),
        message: item.message,
        source: item.source
    }
}

// Actively request diagnostics for a document (the pull model). rust-analyzer
// advertises a diagnosticProvider and only answers pull requests — it never
// pushes textDocument/publishDiagnostics — so without this the official client
// shows nothing (T9 #3 root cause). Return values are distinct on purpose:
//   - null  => "unchanged" report: no update; the caller must KEEP whatever it
//              already rendered (returning [] here would wrongly CLEAR it).
//   - []    => empty "full" report: the file genuinely has no diagnostics; clear.
export async function pullDiagnostics(client: LSPClient, uri: string): Promise<LspDiagnostic[] | null> {
    const report = await client.request<{ textDocument: { uri: string } }, DocumentDiagnosticReport>(
        "textDocument/diagnostic",
        { textDocument: { uri } }
    )
    if (!report || report.kind === "unchanged") return null
    return report.items ?? []
}

// Debounced pull loop. Pulls once on mount, then re-pulls DEBOUNCE_MS after the
// last document change, and dispatches results through setDiagnostics — the same
// lint rendering exit the push path (serverDiagnostics) uses.
class DiagnosticsPullPlugin {
    private pending = -1
    // Bumped on every document change. An in-flight pull captures the generation
    // at request time; if the doc changes before it resolves, the captured value
    // no longer matches and the (now stale) result is discarded — closing both
    // the stale-wins ordering race and the doc-drift window, since any edit both
    // ++generation and schedules a fresh pull.
    private generation = 0

    constructor(
        private readonly view: EditorView,
        private readonly client: LSPClient,
        private readonly uri: string
    ) {
        this.run()
    }

    update(update: ViewUpdate): void {
        if (!update.docChanged) return
        this.generation++
        if (this.pending > -1) clearTimeout(this.pending)
        this.pending = setTimeout(() => {
            this.pending = -1
            this.run()
        }, DEBOUNCE_MS) as unknown as number
    }

    private run(): void {
        const gen = this.generation
        void pullDiagnostics(this.client, this.uri)
            .then((items) => {
                // Unchanged report: keep the currently rendered diagnostics.
                if (items === null) return
                // Superseded by a newer document version while in flight.
                if (gen !== this.generation) return
                this.view.dispatch(
                    setDiagnostics(
                        this.view.state,
                        items.map((item) => toLintDiagnostic(this.view.state.doc, item))
                    )
                )
            })
            .catch(() => {})
    }

    destroy(): void {
        if (this.pending > -1) clearTimeout(this.pending)
    }
}

// Enables the diagnostics pull loop for a file, or an inert (empty) extension
// when the server does not advertise a diagnosticProvider — in which case the
// push path (serverDiagnostics) is left to handle diagnostics unchanged.
export function diagnosticsPullExtension(client: LSPClient, uri: string): Extension {
    if (!client.serverCapabilities?.diagnosticProvider) return []
    return ViewPlugin.define((view) => new DiagnosticsPullPlugin(view, client, uri))
}

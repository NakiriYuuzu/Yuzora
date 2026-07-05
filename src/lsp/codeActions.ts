import type { EditorView } from "@codemirror/view"
import type { Diagnostic } from "@codemirror/lint"
import type { LSPClient } from "@codemirror/lsp-client"

import { forEachDiagnostic } from "@codemirror/lint"

import { workspaceEditToChanges } from "./renameCompat"
import type { WorkspaceEdit } from "./renameCompat"
import { offsetOf, pullDiagnostics, toLintDiagnostic } from "./diagnosticsPull"
import type { LspDiagnostic } from "./diagnosticsPull"
import { strings } from "../lib/i18n"

// A resolved LSP CodeAction, trimmed to the fields we act on. A quick fix
// carries an `edit`; command-only actions leave it undefined.
export interface CodeActionItem {
    title: string
    kind?: string
    edit?: WorkspaceEdit
}

interface RawCodeAction {
    title?: string
    kind?: string
    edit?: WorkspaceEdit
}

// Request code actions for a diagnostic. context.diagnostics MUST carry the
// `message` field: vtsls rejects a codeAction request whose context diagnostics
// omit it (spike finding), so it is forwarded verbatim alongside the range.
export async function codeActionsFor(
    client: LSPClient,
    uri: string,
    diagnostic: LspDiagnostic
): Promise<CodeActionItem[]> {
    // Flush pending didChange first (same as every built-in lsp-client feature and
    // symbols.ts): the user may have typed between the diagnostic render and the
    // quick-fix click, and request() leaves synchronizing to the caller. Without it
    // the server computes the fix against a stale doc, and applyWorkspaceEdit then
    // maps that stale range onto the current doc — a valid-but-misplaced edit the
    // atomic from>to guard can't catch.
    client.sync()
    const result = await client.request<
        {
            textDocument: { uri: string }
            range: LspDiagnostic["range"]
            context: { diagnostics: LspDiagnostic[]; only: string[] }
        },
        RawCodeAction[] | null
    >("textDocument/codeAction", {
        textDocument: { uri },
        range: diagnostic.range,
        context: {
            diagnostics: [
                {
                    range: diagnostic.range,
                    message: diagnostic.message,
                    severity: diagnostic.severity,
                    code: diagnostic.code,
                    source: diagnostic.source
                }
            ],
            // Narrow the server's reply to quick fixes — the sole consumer is the
            // lint quick-fix action, so refactors / source actions are noise here.
            only: ["quickfix"]
        }
    })

    if (!result) return []
    return result
        .filter((a): a is RawCodeAction & { title: string } => a != null && typeof a.title === "string")
        .map((a) => ({ title: a.title, kind: a.kind, edit: a.edit }))
}

// Apply the portion of a WorkspaceEdit that targets `uri` into `view`, mapping
// LSP ranges to CodeMirror offsets and dispatching one transaction. Both wire
// shapes (changes / documentChanges) are supported via the shared T9 mapping.
// Edits for other files are ignored here — cross-file application is the
// workspace's job (YuzoraWorkspace.updateFile).
export function applyWorkspaceEdit(view: EditorView, edit: WorkspaceEdit, uri: string): void {
    // A documentChanges edit can carry several entries for the same uri; collect
    // every matching edit, not just the first (they are doc-ordered and
    // non-overlapping, and each range is an offset into the original doc).
    const edits = workspaceEditToChanges(edit)
        .filter((c) => c.uri === uri)
        .flatMap((c) => c.edits)
    if (edits.length === 0) return

    const doc = view.state.doc
    const changes = edits.map((e) => ({
        from: offsetOf(doc, e.range.start),
        to: offsetOf(doc, e.range.end),
        insert: e.newText
    }))
    // A malformed server can return an inverted range (from > to after offsetOf
    // clamps each end independently); dispatching it throws "Invalid change range".
    // A half-applied WorkspaceEdit is semantically broken, so drop the whole group
    // if any pair is invalid — the same defensive posture as symbols.ts isValidRange.
    if (changes.some((c) => c.from > c.to)) return
    view.dispatch({ changes })
}

// A @codemirror/lint LintSource equivalent to serverDiagnostics, but pull-based
// (calls textDocument/diagnostic) and with each diagnostic given a quick-fix
// action. Activating the action requests code actions for that diagnostic,
// picks the first one carrying an edit, and applies it to the view.
export function lspLintSource(
    client: LSPClient,
    uri: string
): (view: EditorView) => Promise<Diagnostic[]> {
    return async (view) => {
        // Re-read the currently rendered diagnostics at their MAPPED offsets. The
        // Diagnostic objects keep the from/to they were created with; only the lint
        // decoration RangeSet is mapped through edits, so forEachDiagnostic supplies
        // the live positions via its 2nd/3rd args. Dropping them snaps every
        // decoration back to its pre-edit offset after any doc change.
        const keepRendered = (): Diagnostic[] => {
            const kept: Diagnostic[] = []
            forEachDiagnostic(view.state, (d, from, to) => kept.push({ ...d, from, to }))
            return kept
        }
        let items: LspDiagnostic[] | null
        try {
            items = await pullDiagnostics(client, uri)
        } catch {
            // Transient failure (server busy / request cancelled): keep what's
            // shown rather than clearing it or letting the lint plugin log the
            // exception as noise — a push-only server has no diagnosticProvider and
            // is gated out of the linter entirely (assembleLspExtensions).
            return keepRendered()
        }
        // Unchanged report (null): the lint framework has no "no-op" return —
        // returning null/undefined would be concat'd into the diagnostics array
        // and crash rendering — so re-emit the currently rendered diagnostics to
        // leave them untouched.
        if (items === null) return keepRendered()
        const doc = view.state.doc
        return items.map((item) => ({
            ...toLintDiagnostic(doc, item),
            actions: [
                {
                    name: strings.lsp.quickFix,
                    apply: (v: EditorView) => {
                        void codeActionsFor(client, uri, item)
                            .then((actions) => {
                                const fix = actions.find((a) => a.edit)
                                if (fix?.edit) applyWorkspaceEdit(v, fix.edit, uri)
                            })
                            .catch(() => {})
                    }
                }
            ]
        }))
    }
}

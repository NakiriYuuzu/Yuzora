import { languageServerSupport } from "@codemirror/lsp-client"
import { linter } from "@codemirror/lint"
import type { Extension } from "@codemirror/state"

import { lspLintSource } from "./codeActions"
import { semanticTokensExtension } from "./semanticTokens"
import type { ManagedClient } from "./lspManager"
import { pathToUri } from "./workspace"

// Assembles the CodeMirror extensions for one open file's LSP integration. Called
// from lspExtensionsForFile (lspManager) AFTER the initialize handshake, so
// client.serverCapabilities is populated and the capability gates below see the
// real provider set (A0).
//
// Baseline: languageServerSupport (hover, definitions, references, rename,
// completion, signature help). Layered on top:
//   - semanticTokensExtension: highlighting over the Lezer baseline (gated on
//     semanticTokensProvider inside that function).
//   - linter(lspLintSource): pull-mode diagnostics WITH quick-fix actions, added
//     ONLY for a server that advertises a diagnosticProvider (rust-analyzer,
//     vtsls). Same defensive gate as semanticTokensExtension — without it a
//     push-only server (typescript-language-server) is sent a doomed
//     textDocument/diagnostic every lint cycle, whose failure the lint plugin
//     would log as noise.
//
// The two diagnostic channels are split by server model:
//   - Push (textDocument/publishDiagnostics): wired via serverDiagnostics() on the
//     LSPClient itself (lspManager.startClient), because it is an LSPClientExtension
//     — a notification handler + its lint rendering — NOT a plain editor Extension.
//     languageServerSupport surfaces its editorExtension through LSPPlugin.create.
//   - Pull (textDocument/diagnostic + quick fixes): the gated linter here, the sole
//     quick-fix path. T9 diagnosticsPullExtension is intentionally left unwired —
//     the linter is its superset (same pull, plus fixes), so wiring both would
//     double-render.
//
// Known limitation: a both-capable server (pushes AND advertises diagnosticProvider)
// can double-render; accepted, verified on real servers in T15.
//
// Note: languageServerSupport is marked deprecated in the 6.2.5 d.ts in favour of
// LSPPlugin.create + à-la-carte extensions, but it remains the documented "bundle
// everything" entry point and is what the plan specifies for the M3 baseline.
export function assembleLspExtensions(client: ManagedClient, path: string): Extension[] {
    const uri = pathToUri(path)
    const extensions: Extension[] = [
        languageServerSupport(client.client, uri),
        semanticTokensExtension(client.client, uri)
    ]
    if (client.client.serverCapabilities?.diagnosticProvider) {
        extensions.push(linter(lspLintSource(client.client, uri)))
    }
    return extensions
}

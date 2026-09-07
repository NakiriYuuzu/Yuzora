import { LSPPlugin, serverDiagnostics, type LSPClientExtension } from "@codemirror/lsp-client"

/** Keep the library's version checks, change mapping and autosync, but reject
 * stale unversioned ranges before they can throw inside its notification handler. */
export function guardedServerDiagnostics(): LSPClientExtension {
    const extension = serverDiagnostics()
    const deliver = extension.notificationHandlers!["textDocument/publishDiagnostics"]
    return {
        ...extension,
        notificationHandlers: {
            "textDocument/publishDiagnostics": (client, params) => {
                const file = client.workspace.getFile(params.uri)
                const view = file?.getView()
                const plugin = view && LSPPlugin.get(view)
                if (!plugin) return deliver(client, params)
                const doc = plugin.syncedDoc
                const valid = (position: { line: number; character: number }) => position
                    && Number.isInteger(position.line) && Number.isInteger(position.character)
                    && position.line >= 0 && position.line < doc.lines
                    && position.character >= 0 && position.character <= doc.line(position.line + 1).length
                // vtsls can publish diagnostics without a version after an undo
                // shortened the synced document. Preserve rendered diagnostics
                // until a valid report arrives, rather than terminating the LSP.
                if (!Array.isArray(params.diagnostics) || params.diagnostics.some((item: {
                    range?: { start: { line: number; character: number }; end: { line: number; character: number } }
                }) => !item.range || !valid(item.range.start) || !valid(item.range.end)
                    || item.range.start.line > item.range.end.line
                    || (item.range.start.line === item.range.end.line && item.range.start.character > item.range.end.character))) return true
                return deliver(client, params)
            }
        }
    }
}

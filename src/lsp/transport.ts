import type { Transport } from "@codemirror/lsp-client"

import { lspSend, lspStart } from "../lib/ipc"
import type { LspServerInfo } from "../lib/types"

// Bridges @codemirror/lsp-client's Transport to the Rust-side LSP process over
// the IPC layer (T4). One transport == one (workspace, language) server:
//   - construction calls lspStart, wiring the Channel onMessage callback to a
//     fan-out dispatcher over all subscribed handlers.
//   - send() forwards raw JSON-RPC (no LSP headers) via lspSend.
//
// Workspace-path consistency (wave 1 review handoff): the Rust side keys its
// process map on the raw `workspace` string. lspStart / lspSend here forward
// exactly the string given by the caller — no canonicalize / normalize — so a
// single client must always pass the same workspace string it was created with.
export interface TransportHandle {
    transport: Transport
    dispose: () => void
    info: Promise<LspServerInfo>
}

export function createTauriTransport(workspace: string, language: string): TransportHandle {
    const handlers = new Set<(value: string) => void>()
    const dispatch = (message: string) => {
        for (const handler of handlers) handler(message)
    }

    const info = lspStart(workspace, language, dispatch)
    // Guard against a floating unhandled rejection when nobody consumes `info`
    // (e.g. lspStart fails and the caller ignores the handle). Consumers can
    // still await `info` — multiple handlers all fire.
    info.catch(() => {})

    const transport: Transport = {
        send(message: string) {
            // Swallow send failures: once the server process is gone every send
            // would otherwise raise an unhandled rejection storm. Status surfaces
            // separately via lsp:server-status (W7).
            void lspSend(workspace, language, message).catch(() => {})
        },
        subscribe(handler: (value: string) => void) {
            handlers.add(handler)
        },
        unsubscribe(handler: (value: string) => void) {
            handlers.delete(handler)
        }
    }

    return {
        transport,
        dispose() {
            handlers.clear()
        },
        info
    }
}

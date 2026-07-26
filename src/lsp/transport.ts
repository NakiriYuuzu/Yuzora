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

    // Ordering guarantee (#56 review fix): lsp_send / lsp_start are async
    // commands since T2 — each invoke runs on its own blocking-pool task, so
    // two in-flight sends could reach the server's stdin out of order (the
    // Rust-side mutex only prevents interleaving, not reordering). LSP requires
    // client messages in order — an out-of-order didChange silently corrupts
    // the server's document state. Serialize per transport: every send waits
    // for the previous one (and for lspStart, so initialize can't overtake
    // server startup) to settle before invoking.
    let sendChain: Promise<unknown> = info.catch(() => {})

    const transport: Transport = {
        send(message: string) {
            // Swallow send failures per link: once the server process is gone
            // every send would otherwise raise an unhandled rejection storm —
            // and a stuck chain would drop all later messages. Status surfaces
            // separately via lsp:server-status (W7).
            sendChain = sendChain
                .then(() => lspSend(workspace, language, message))
                .catch(() => {})
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

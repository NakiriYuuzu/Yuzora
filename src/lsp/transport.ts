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

export function createTauriTransport(workspace: string, language: string, onFailure?: (reason: string) => void): TransportHandle {
    const handlers = new Set<(value: string) => void>()
    let disposed = false
    let sending = false
    let queuedBytes = 0
    const queue: Array<{ message: string; bytes: number }> = []
    const clear = () => {
        disposed = true
        handlers.clear()
        queue.length = 0
        queuedBytes = 0
    }
    const fail = (reason: string) => {
        if (disposed) return
        clear()
        onFailure?.(reason)
    }
    const dispatch = (message: string) => {
        if (disposed) return
        for (const handler of handlers) handler(message)
    }

    const info = lspStart(workspace, language, dispatch)
    // Guard against a floating unhandled rejection when nobody consumes `info`
    // (e.g. lspStart fails and the caller ignores the handle). Consumers can
    // still await `info` — multiple handlers all fire.
    info.catch((error) => fail(String(error)))

    // Ordering guarantee (#56 review fix): lsp_send / lsp_start are async
    // commands since T2 — each invoke runs on its own blocking-pool task, so
    // two in-flight sends could reach the server's stdin out of order (the
    // Rust-side mutex only prevents interleaving, not reordering). LSP requires
    // client messages in order — an out-of-order didChange silently corrupts
    // the server's document state. Serialize per transport: every send waits
    // for the previous one (and for lspStart, so initialize can't overtake
    // server startup) to settle before invoking.
    const drain = async () => {
        if (sending || disposed) return
        sending = true
        try {
            await info
            while (!disposed && queue.length > 0) {
                const next = queue[0]
                await lspSend(workspace, language, next.message)
                if (disposed) break
                queue.shift()
                queuedBytes -= next.bytes
            }
        } catch (error) {
            // A lost didChange makes all subsequent offsets invalid. Clear the
            // bounded queue and require a fresh initialize/didOpen on restart.
            fail(String(error))
        } finally {
            sending = false
        }
    }

    const transport: Transport = {
        send(message: string) {
            if (disposed) return
            const maxMessage = 8 * 1024 * 1024
            if (message.length > maxMessage) { fail("LSP message limit exceeded"); return }
            const bytes = new TextEncoder().encode(message).byteLength
            if (bytes > maxMessage || queuedBytes + bytes > 16 * 1024 * 1024 || queue.length >= 128) {
                fail("LSP send queue limit exceeded")
                return
            }
            queue.push({ message, bytes })
            queuedBytes += bytes
            void drain()
        },
        subscribe(handler: (value: string) => void) {
            if (!disposed) handlers.add(handler)
        },
        unsubscribe(handler: (value: string) => void) {
            handlers.delete(handler)
        }
    }

    return {
        transport,
        dispose() {
            clear()
        },
        info
    }
}

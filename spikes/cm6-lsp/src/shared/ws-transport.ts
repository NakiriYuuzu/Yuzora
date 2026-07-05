import type { Transport } from '@codemirror/lsp-client'
import { logIn, logOut } from './log'

// Transport for @codemirror/lsp-client: messages are raw JSON strings, no
// LSP Content-Length framing (the bridge in bridge/ws-stdio-bridge.ts adds
// and strips that on the stdio side). Wrapping send/receive here logs every
// protocol message to the console as spike evidence.
export function loggingWebSocketTransport(uri: string): Promise<Transport> {
    let handlers: ((value: string) => void)[] = []
    const sock = new WebSocket(uri)
    sock.onmessage = (e) => {
        const text = e.data.toString()
        try {
            const parsed = JSON.parse(text)
            logIn(parsed.method, parsed)
        } catch {
            logIn(undefined, text)
        }
        for (const h of handlers) h(text)
    }
    return new Promise((resolve, reject) => {
        sock.onerror = (e) => reject(e)
        sock.onopen = () =>
            resolve({
                send(message: string) {
                    try {
                        const parsed = JSON.parse(message)
                        logOut(parsed.method, parsed)
                    } catch {
                        logOut(undefined, message)
                    }
                    sock.send(message)
                },
                subscribe(handler: (value: string) => void) {
                    handlers.push(handler)
                },
                unsubscribe(handler: (value: string) => void) {
                    handlers = handlers.filter((h) => h !== handler)
                },
            })
    })
}

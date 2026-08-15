import {
    WebSocketTransport,
    type JSONRPCMessage,
    type Transport,
} from '@marimo-team/codemirror-languageserver'
import { logIn, logOut } from './log'

// v2 owns its JSON-RPC transport contract, so wrap the package transport and
// log parsed messages at that boundary instead of subclassing @open-rpc.
class LoggingWebSocketTransport implements Transport {
    private readonly transport: WebSocketTransport

    constructor(uri: string) {
        this.transport = new WebSocketTransport(uri)
    }

    connect() {
        return this.transport.connect()
    }

    send(message: JSONRPCMessage) {
        logOut(undefined, message)
        this.transport.send(message)
    }

    onMessage(handler: (message: JSONRPCMessage) => void) {
        return this.transport.onMessage((message) => {
            logIn(undefined, message)
            handler(message)
        })
    }

    onClose(handler: (error: Error) => void) {
        return this.transport.onClose(handler)
    }

    close() {
        this.transport.close()
    }
}

export function loggingMarimoTransport(uri: string) {
    return new LoggingWebSocketTransport(uri)
}

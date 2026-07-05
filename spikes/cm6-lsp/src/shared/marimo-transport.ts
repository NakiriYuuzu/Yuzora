import { WebSocketTransport } from '@open-rpc/client-js'
import { logIn, logOut } from './log'

// @marimo-team/codemirror-languageserver drives its LanguageServerClient
// through an @open-rpc/client-js Transport. Subclassing (rather than
// monkey-patching) gives access to the protected parseData() helper so the
// logged payload matches the actual wire JSON-RPC message.
class LoggingWebSocketTransport extends WebSocketTransport {
    async sendData(data: any, timeout: number | null = 5000) {
        try {
            logOut(undefined, this.parseData(data))
        } catch {}
        return super.sendData(data, timeout)
    }
}

export function loggingMarimoTransport(uri: string) {
    const transport = new LoggingWebSocketTransport(uri)
    transport.connection.addEventListener('message', (e: MessageEvent) => {
        try {
            logIn(undefined, JSON.parse(e.data))
        } catch {
            logIn(undefined, e.data)
        }
    })
    return transport
}

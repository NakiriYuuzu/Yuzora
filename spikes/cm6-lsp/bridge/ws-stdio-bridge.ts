// Minimal WebSocket <-> stdio bridge for LSP servers (spike, throwaway).
// Browser side sends/receives raw JSON strings (no LSP headers), per the
// @codemirror/lsp-client Transport contract. This bridge adds/strips the
// `Content-Length` framing required by stdio-based LSP servers.
//
// Usage:
//   LSP_CMD="node_modules/.bin/vtsls --stdio" LSP_CWD=$(pwd)/fixtures/ts-app BRIDGE_PORT=5198 bun run bridge/ws-stdio-bridge.ts
//   LSP_CMD="rust-analyzer" LSP_CWD=$(pwd)/fixtures/rs-app BRIDGE_PORT=5197 bun run bridge/ws-stdio-bridge.ts

const port = Number(process.env.BRIDGE_PORT ?? 5198)
const cmd = process.env.LSP_CMD
const cwd = process.env.LSP_CWD ?? process.cwd()

if (!cmd) {
    console.error('LSP_CMD env var is required, e.g. LSP_CMD="rust-analyzer"')
    process.exit(1)
}

const cmdParts = cmd.split(' ')

function frame(json: string) {
    const body = Buffer.from(json, 'utf8')
    return Buffer.concat([Buffer.from(`Content-Length: ${body.length}\r\n\r\n`, 'utf8'), body])
}

function createStdioReader(onMessage: (json: string) => void) {
    let buf = Buffer.alloc(0)
    return (chunk: Uint8Array) => {
        buf = Buffer.concat([buf, Buffer.from(chunk)])
        while (true) {
            const headerEnd = buf.indexOf('\r\n\r\n')
            if (headerEnd === -1) return
            const header = buf.subarray(0, headerEnd).toString('utf8')
            const match = header.match(/Content-Length: (\d+)/i)
            if (!match) {
                console.error('[bridge] malformed header, dropping buffer:', header)
                buf = Buffer.alloc(0)
                return
            }
            const length = Number(match[1])
            const bodyStart = headerEnd + 4
            if (buf.length < bodyStart + length) return
            const body = buf.subarray(bodyStart, bodyStart + length).toString('utf8')
            buf = buf.subarray(bodyStart + length)
            onMessage(body)
        }
    }
}

console.log(`[bridge] cmd="${cmd}" cwd=${cwd} port=${port}`)

Bun.serve({
    port,
    fetch(req, server) {
        if (server.upgrade(req)) return
        return new Response('ws-stdio-bridge: connect with a WebSocket client', { status: 400 })
    },
    websocket: {
        open(ws) {
            console.log('[bridge] client connected, spawning LSP server')
            const proc = Bun.spawn(cmdParts, {
                cwd,
                stdin: 'pipe',
                stdout: 'pipe',
                stderr: 'inherit',
            })
            ;(ws.data as any) = { proc }

            const reader = createStdioReader((json) => {
                ws.send(json)
            })

            const pump = async () => {
                const stream = proc.stdout as ReadableStream<Uint8Array>
                const streamReader = stream.getReader()
                while (true) {
                    const { value, done } = await streamReader.read()
                    if (done) break
                    reader(value)
                }
            }
            pump().catch((err) => console.error('[bridge] stdout pump error', err))

            proc.exited.then((code) => {
                console.log(`[bridge] LSP server exited with code ${code}`)
                try {
                    ws.close()
                } catch {}
            })
        },
        message(ws, message) {
            const proc = (ws.data as any)?.proc
            if (!proc) return
            const json = typeof message === 'string' ? message : Buffer.from(message).toString('utf8')
            proc.stdin.write(frame(json))
            proc.stdin.flush()
        },
        close(ws) {
            const proc = (ws.data as any)?.proc
            if (proc) proc.kill()
            console.log('[bridge] client disconnected, killed LSP server')
        },
    },
})

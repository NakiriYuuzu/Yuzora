// Rerunnable demo: drives vtsls directly over stdio (no browser, no CodeMirror)
// to prove the code actions chain end to end -- request diagnostics, request
// textDocument/codeAction, receive the quickfix WorkspaceEdit, map it onto the
// document text, and apply it. Written for Task 2 review finding: the earlier
// browser-driven demo (evidence/08-code-action-applied.png) had no dedicated
// console log and no script that could be rerun to reproduce it.
//
// Run:  cd spikes/cm6-lsp && bun run scripts/demo-code-action.ts
// Output: printed to stdout AND written to evidence/console-08-code-action-apply.log
//
// Does NOT modify fixtures/ts-app/src/b.ts on disk. The WorkspaceEdit returned
// by vtsls is applied to an in-memory copy of the fixture text only, so the
// fixture keeps its intentional unused `Greeting` type import and this script
// stays rerunnable with the same result. Exits non-zero on any failed
// assertion so a rerun's pass/fail is unambiguous.

import { resolve, dirname } from 'node:path'
import { fileURLToPath } from 'node:url'

const SCRIPT_DIR = dirname(fileURLToPath(import.meta.url))
const SPIKE_ROOT = resolve(SCRIPT_DIR, '..')
const FIXTURE_ROOT = resolve(SPIKE_ROOT, 'fixtures/ts-app')
const VTSLS_BIN = resolve(SPIKE_ROOT, 'node_modules/.bin/vtsls')
const B_TS_PATH = resolve(FIXTURE_ROOT, 'src/b.ts')
const LOG_PATH = resolve(SPIKE_ROOT, 'evidence/console-08-code-action-apply.log')

const bUri = `file://${B_TS_PATH}`
const rootUri = `file://${FIXTURE_ROOT}`

const logLines: string[] = []
function log(...parts: unknown[]) {
    const text = parts.map((p) => (typeof p === 'string' ? p : JSON.stringify(p))).join(' ')
    console.log(text)
    logLines.push(text)
}

// Same Content-Length framing as bridge/ws-stdio-bridge.ts, reused here
// because this script talks to vtsls's stdio directly (no WebSocket hop).
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

type PendingEntry = { resolve: (v: any) => void; reject: (e: any) => void }

class LspProcessClient {
    proc: ReturnType<typeof Bun.spawn>
    private nextId = 1
    private pending = new Map<number, PendingEntry>()
    diagnosticsByUri = new Map<string, any[]>()

    constructor() {
        this.proc = Bun.spawn([VTSLS_BIN, '--stdio'], {
            cwd: FIXTURE_ROOT,
            stdin: 'pipe',
            stdout: 'pipe',
            stderr: 'inherit',
        })
        const reader = createStdioReader((json) => this.handleMessage(json))
        this.pump(reader)
    }

    private async pump(reader: (chunk: Uint8Array) => void) {
        const stream = this.proc.stdout as ReadableStream<Uint8Array>
        const streamReader = stream.getReader()
        while (true) {
            const { value, done } = await streamReader.read()
            if (done) break
            reader(value)
        }
    }

    private handleMessage(json: string) {
        const msg = JSON.parse(json)
        if (msg.id !== undefined && (msg.result !== undefined || msg.error !== undefined)) {
            log('[LSP<] response', msg.id, json)
            const entry = this.pending.get(msg.id)
            if (entry) {
                this.pending.delete(msg.id)
                if (msg.error) entry.reject(new Error(JSON.stringify(msg.error)))
                else entry.resolve(msg.result)
            }
            return
        }
        if (msg.method && msg.id !== undefined) {
            // Server -> client request. Ack generically so vtsls doesn't stall
            // waiting for e.g. client/registerCapability or workspace/configuration.
            log('[LSP<] server request', msg.method, json)
            if (msg.method === 'workspace/configuration') {
                const items = msg.params?.items ?? []
                this.write({ jsonrpc: '2.0', id: msg.id, result: items.map(() => null) })
            } else {
                this.write({ jsonrpc: '2.0', id: msg.id, result: null })
            }
            return
        }
        if (msg.method) {
            log('[LSP<] notification', msg.method, json)
            if (msg.method === 'textDocument/publishDiagnostics') {
                this.diagnosticsByUri.set(msg.params.uri, msg.params.diagnostics)
            }
            return
        }
    }

    private write(obj: unknown) {
        const json = JSON.stringify(obj)
        this.proc.stdin!.write(frame(json))
        ;(this.proc.stdin as any).flush?.()
    }

    request(method: string, params: unknown, timeoutMs = 20000): Promise<any> {
        const id = this.nextId++
        const payload = { jsonrpc: '2.0', id, method, params }
        log('[LSP>] request', method, JSON.stringify(payload))
        this.write(payload)
        return new Promise((resolvePromise, rejectPromise) => {
            const timer = setTimeout(() => {
                this.pending.delete(id)
                rejectPromise(new Error(`timeout waiting for response to ${method} (id=${id})`))
            }, timeoutMs)
            this.pending.set(id, {
                resolve: (v) => {
                    clearTimeout(timer)
                    resolvePromise(v)
                },
                reject: (e) => {
                    clearTimeout(timer)
                    rejectPromise(e)
                },
            })
        })
    }

    notify(method: string, params: unknown) {
        const payload = { jsonrpc: '2.0', method, params }
        log('[LSP>] notification', method, JSON.stringify(payload))
        this.write(payload)
    }

    async waitForDiagnostics(uri: string, timeoutMs = 15000): Promise<any[]> {
        const start = Date.now()
        while (Date.now() - start < timeoutMs) {
            const existing = this.diagnosticsByUri.get(uri)
            if (existing) return existing
            await new Promise((r) => setTimeout(r, 200))
        }
        throw new Error(`timed out waiting for publishDiagnostics on ${uri}`)
    }
}

// LSP positions are UTF-16 code unit line/character offsets. The fixture is
// pure ASCII, so JS string indices line up with UTF-16 code units and this
// plain split-and-count conversion is exact (no surrogate-pair handling needed).
function offsetAt(text: string, pos: { line: number; character: number }): number {
    const docLines = text.split('\n')
    let offset = 0
    for (let i = 0; i < pos.line; i++) offset += docLines[i].length + 1
    return offset + pos.character
}

function applyTextEdits(original: string, edits: Array<{ range: any; newText: string }>): string {
    const sorted = [...edits].sort((a, b) => offsetAt(original, b.range.start) - offsetAt(original, a.range.start))
    let result = original
    for (const edit of sorted) {
        const start = offsetAt(result, edit.range.start)
        const end = offsetAt(result, edit.range.end)
        result = result.slice(0, start) + edit.newText + result.slice(end)
    }
    return result
}

async function main() {
    log('=== demo-code-action.ts start', new Date().toISOString(), '===')
    log('fixture:', B_TS_PATH)

    let client: LspProcessClient | undefined
    let exitCode = 0

    try {
        client = new LspProcessClient()
        log('[proc] spawned vtsls, pid =', client.proc.pid)

        const initResult = await client.request('initialize', {
            processId: process.pid,
            rootUri,
            workspaceFolders: [{ uri: rootUri, name: 'ts-app' }],
            capabilities: {
                textDocument: {
                    synchronization: { didSave: true },
                    publishDiagnostics: { relatedInformation: true },
                    codeAction: {
                        codeActionLiteralSupport: {
                            codeActionKind: {
                                valueSet: [
                                    'quickfix',
                                    'refactor',
                                    'refactor.extract',
                                    'refactor.inline',
                                    'refactor.rewrite',
                                    'source',
                                    'source.organizeImports',
                                ],
                            },
                        },
                        isPreferredSupport: true,
                    },
                },
                workspace: {
                    applyEdit: true,
                    workspaceEdit: { documentChanges: false },
                },
            },
        })
        log(
            '[step] initialize done, serverCapabilities.codeActionProvider =',
            JSON.stringify(initResult.capabilities?.codeActionProvider),
        )
        client.notify('initialized', {})

        const originalText = await Bun.file(B_TS_PATH).text()
        log('[step] read fixture from disk, length =', originalText.length)

        client.notify('textDocument/didOpen', {
            textDocument: { uri: bUri, languageId: 'typescript', version: 1, text: originalText },
        })
        log('[step] didOpen sent for', bUri)

        const diagnostics = await client.waitForDiagnostics(bUri)
        log('[step] received', diagnostics.length, 'diagnostics for b.ts')
        const unusedImportDiag = diagnostics.find(
            (d: any) => typeof d.message === 'string' && d.message.includes('Greeting'),
        )
        if (!unusedImportDiag) {
            throw new Error(`no diagnostic mentioning 'Greeting' found; got: ${JSON.stringify(diagnostics)}`)
        }
        log('[step] target diagnostic:', JSON.stringify(unusedImportDiag))

        const actions = await client.request('textDocument/codeAction', {
            textDocument: { uri: bUri },
            range: unusedImportDiag.range,
            context: { diagnostics: [unusedImportDiag] },
        })
        log('[step] received', Array.isArray(actions) ? actions.length : 0, 'code actions:', JSON.stringify(actions))

        const actionList = Array.isArray(actions) ? actions : []
        const chosen =
            actionList.find((a) => a.isPreferred) ??
            actionList.find((a) => typeof a.title === 'string' && a.title.includes('Remove unused')) ??
            actionList[0]
        if (!chosen) throw new Error('no code action returned by vtsls')
        log(
            '[step] chosen action:',
            JSON.stringify({ title: chosen.title, kind: chosen.kind, isPreferred: chosen.isPreferred }),
        )

        const workspaceEdit = chosen.edit
        if (!workspaceEdit?.changes?.[bUri]) {
            throw new Error(
                `WorkspaceEdit missing changes for ${bUri}; got keys: ${JSON.stringify(Object.keys(workspaceEdit?.changes ?? {}))}`,
            )
        }
        const edits = workspaceEdit.changes[bUri]
        log('[step] WorkspaceEdit.changes[bUri]:', JSON.stringify(edits))

        const patchedText = applyTextEdits(originalText, edits)

        log('=== BEFORE (fixtures/ts-app/src/b.ts, on disk, unchanged by this script) ===')
        log(originalText)
        log('=== AFTER (in-memory only -- WorkspaceEdit applied, fixture on disk left untouched) ===')
        log(patchedText)

        // Check for the standalone `type Greeting` specifier, not the substring
        // "Greeting" -- `buildGreeting`/`repeatGreeting` legitimately contain it.
        const patchedImportLine = patchedText.split('\n').find((l) => l.startsWith('import'))
        const stillHasUnusedType = patchedImportLine?.includes('type Greeting') ?? true
        const stillHasUsedImports =
            !!patchedImportLine?.includes('buildGreeting') && !!patchedImportLine?.includes('repeatGreeting')
        if (stillHasUnusedType || !stillHasUsedImports) {
            throw new Error(`assertion failed: unexpected import line after patch: ${patchedImportLine}`)
        }
        log('[assert] OK: unused "type Greeting" import removed, used imports preserved')

        await client.request('shutdown', null)
        client.notify('exit', {})
        log('[step] shutdown/exit sent')
    } catch (err) {
        exitCode = 1
        log('[demo-code-action] FAILED:', String((err as Error)?.stack ?? err))
    } finally {
        if (client) {
            try {
                client.proc.kill()
            } catch {}
            const code = await client.proc.exited.catch(() => -1)
            log('[proc] vtsls exited with code', code)
        }
        await Bun.write(LOG_PATH, logLines.join('\n') + '\n')
        console.log('=== demo-code-action.ts done, log written to', LOG_PATH, '===')
    }

    process.exit(exitCode)
}

main()

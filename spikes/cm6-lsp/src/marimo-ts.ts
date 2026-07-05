import { EditorView, basicSetup } from 'codemirror'
import { javascript } from '@codemirror/lang-javascript'
import { LanguageServerClient, languageServerWithClient } from '@marimo-team/codemirror-languageserver'
import { loggingMarimoTransport } from './shared/marimo-transport'
import { expose } from './shared/log'
import aSource from '../fixtures/ts-app/src/a.ts?raw'
import bSource from '../fixtures/ts-app/src/b.ts?raw'

// Spike-only: hardcoded absolute path, see src/official-ts.ts for rationale.
const FIXTURE_ROOT = 'file:///Users/yuuzu/HanaokaYuuzu/App/Tauri/yuzora/spikes/cm6-lsp/fixtures/ts-app'
const BRIDGE_PORT = 5198 // vtsls; the bridge spawns a fresh server per WS connection

const statusEl = document.querySelector('#status')!

async function main() {
    statusEl.textContent = `connecting to ws://localhost:${BRIDGE_PORT} …`
    const transport = loggingMarimoTransport(`ws://localhost:${BRIDGE_PORT}`)

    const client = new LanguageServerClient({
        rootUri: FIXTURE_ROOT,
        workspaceFolders: [{ uri: FIXTURE_ROOT, name: 'ts-app' }],
        transport,
        // Finding (spike): this package's default capabilities omit
        // textDocument.publishDiagnostics. vtsls then never pushes diagnostics
        // notifications at all. Adding it back restores diagnostics push.
        capabilities: (defaults) => ({
            ...defaults,
            textDocument: {
                ...defaults?.textDocument,
                publishDiagnostics: { versionSupport: true },
            },
        }),
    })
    await client.initializePromise

    const caps: any = client.capabilities
    statusEl.textContent =
        `connected (marimo). semanticTokensProvider=${Boolean(caps?.semanticTokensProvider)} ` +
        `codeActionProvider=${Boolean(caps?.codeActionProvider)} renameProvider=${Boolean(caps?.renameProvider)}`
    console.log('[spike] marimo serverCapabilities', JSON.stringify(caps))

    const viewA = new EditorView({
        doc: aSource,
        extensions: [
            basicSetup,
            javascript({ typescript: true }),
            languageServerWithClient({
                client,
                documentUri: `${FIXTURE_ROOT}/src/a.ts`,
                languageId: 'typescript',
            }),
        ],
        parent: document.querySelector('#editor-a')!,
    })
    const viewB = new EditorView({
        doc: bSource,
        extensions: [
            basicSetup,
            javascript({ typescript: true }),
            languageServerWithClient({
                client,
                documentUri: `${FIXTURE_ROOT}/src/b.ts`,
                languageId: 'typescript',
            }),
        ],
        parent: document.querySelector('#editor-b')!,
    })

    expose('client', client)
    expose('viewA', viewA)
    expose('viewB', viewB)
    console.log('[spike] marimo ready')
}

main().catch((err) => {
    statusEl.textContent = `connection failed: ${err}`
    console.error('[spike] marimo init failed', err)
})

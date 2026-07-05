import { EditorView, basicSetup } from 'codemirror'
import { rust } from '@codemirror/lang-rust'
import { keymap } from '@codemirror/view'
import {
    LSPClient,
    languageServerExtensions,
    jumpToDefinition,
    jumpToDefinitionKeymap,
    renameSymbol,
    renameKeymap,
} from '@codemirror/lsp-client'
import { loggingWebSocketTransport } from './shared/ws-transport'
import { expose } from './shared/log'
import mainSource from '../fixtures/rs-app/src/main.rs?raw'
import greetingSource from '../fixtures/rs-app/src/greeting.rs?raw'

// Spike-only: hardcoded absolute path, see src/official-ts.ts for rationale.
const FIXTURE_ROOT = 'file:///Users/yuuzu/HanaokaYuuzu/App/Tauri/yuzora/spikes/cm6-lsp/fixtures/rs-app'
const BRIDGE_PORT = 5197 // rust-analyzer

const statusEl = document.querySelector('#status')!

async function main() {
    statusEl.textContent = `connecting to ws://localhost:${BRIDGE_PORT} …`
    const transport = await loggingWebSocketTransport(`ws://localhost:${BRIDGE_PORT}`)

    const client = new LSPClient({
        rootUri: FIXTURE_ROOT,
        extensions: languageServerExtensions(),
        timeout: 20000, // rust-analyzer's first index can be slow
    }).connect(transport)

    await client.initializing
    const caps = client.serverCapabilities
    statusEl.textContent =
        `connected. semanticTokensProvider=${Boolean(caps?.semanticTokensProvider)} ` +
        `codeActionProvider=${Boolean(caps?.codeActionProvider)} ` +
        `renameProvider=${Boolean(caps?.renameProvider)}`
    console.log('[spike] rust serverCapabilities', JSON.stringify(caps))

    const viewA = new EditorView({
        doc: mainSource,
        extensions: [
            basicSetup,
            rust(),
            client.plugin(`${FIXTURE_ROOT}/src/main.rs`),
            keymap.of([...jumpToDefinitionKeymap, ...renameKeymap]),
        ],
        parent: document.querySelector('#editor-a')!,
    })
    const viewB = new EditorView({
        doc: greetingSource,
        extensions: [
            basicSetup,
            rust(),
            client.plugin(`${FIXTURE_ROOT}/src/greeting.rs`),
            keymap.of([...jumpToDefinitionKeymap, ...renameKeymap]),
        ],
        parent: document.querySelector('#editor-b')!,
    })

    expose('client', client)
    expose('viewA', viewA)
    expose('viewB', viewB)
    expose('jumpToDefinition', jumpToDefinition)
    expose('renameSymbol', renameSymbol)
    console.log('[spike] rust ready')
}

main().catch((err) => {
    statusEl.textContent = `connection failed: ${err}`
    console.error('[spike] rust init failed', err)
})

import { EditorView, basicSetup } from 'codemirror'
import { javascript } from '@codemirror/lang-javascript'
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
import aSource from '../fixtures/ts-app/src/a.ts?raw'
import bSource from '../fixtures/ts-app/src/b.ts?raw'

// Spike-only: hardcoded absolute path so vtsls (spawned with matching cwd
// by the bridge) can resolve the real files on disk for cross-file module
// resolution. Not meant to be portable.
const FIXTURE_ROOT = 'file:///Users/yuuzu/HanaokaYuuzu/App/Tauri/yuzora/spikes/cm6-lsp/fixtures/ts-app'
const BRIDGE_PORT = 5198 // vtsls, see spikes/cm6-lsp/README (port plan)

const statusEl = document.querySelector('#status')!

async function main() {
    statusEl.textContent = `connecting to ws://localhost:${BRIDGE_PORT} …`
    const transport = await loggingWebSocketTransport(`ws://localhost:${BRIDGE_PORT}`)

    const client = new LSPClient({
        rootUri: FIXTURE_ROOT,
        extensions: languageServerExtensions(),
    }).connect(transport)

    await client.initializing
    const caps = client.serverCapabilities
    statusEl.textContent =
        `connected. semanticTokensProvider=${Boolean(caps?.semanticTokensProvider)} ` +
        `codeActionProvider=${Boolean(caps?.codeActionProvider)} ` +
        `renameProvider=${Boolean(caps?.renameProvider)}`
    console.log('[spike] serverCapabilities', JSON.stringify(caps))

    const viewA = new EditorView({
        doc: aSource,
        extensions: [
            basicSetup,
            javascript({ typescript: true }),
            client.plugin(`${FIXTURE_ROOT}/src/a.ts`),
            keymap.of([...jumpToDefinitionKeymap, ...renameKeymap]),
        ],
        parent: document.querySelector('#editor-a')!,
    })
    const viewB = new EditorView({
        doc: bSource,
        extensions: [
            basicSetup,
            javascript({ typescript: true }),
            client.plugin(`${FIXTURE_ROOT}/src/b.ts`),
            keymap.of([...jumpToDefinitionKeymap, ...renameKeymap]),
        ],
        parent: document.querySelector('#editor-b')!,
    })

    expose('client', client)
    expose('viewA', viewA)
    expose('viewB', viewB)
    expose('jumpToDefinition', jumpToDefinition)
    expose('renameSymbol', renameSymbol)
    console.log('[spike] ready')
}

main().catch((err) => {
    statusEl.textContent = `connection failed: ${err}`
    console.error('[spike] init failed', err)
})

import { defineConfig } from 'vite'

export default defineConfig({
    server: {
        port: 5199,
        strictPort: true,
    },
    resolve: {
        // @open-rpc/client-js (used by @marimo-team/codemirror-languageserver)
        // imports Node's "events" module. Vite externalizes Node builtins by
        // default in browser code; redirect to the "events" npm polyfill.
        alias: {
            events: 'events/',
        },
    },
})

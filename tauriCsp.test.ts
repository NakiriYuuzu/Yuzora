import { createHash } from "node:crypto"
import { readFileSync } from "node:fs"
import { resolve } from "node:path"
import react from "@vitejs/plugin-react"
import { describe, expect, it } from "vitest"
import config from "./src-tauri/tauri.conf.json"

const { security } = config.app
const tokens = (directive: string) => directive.split(/\s+/)
const hash = (script: string) => `'sha256-${createHash("sha256").update(script.replace(/\r\n?/g, "\n")).digest("base64")}'`

describe.each(["csp", "devCsp"] as const)("Tauri %s", (name) => {
    const policy = security[name]

    it("limits scripts, connections and workers to the main webview inventory", () => {
        expect(policy["default-src"]).toBe("'self'")
        expect(tokens(policy["script-src"])).toContain("'self'")
        expect(tokens(policy["script-src"])).not.toContain("'unsafe-inline'")
        expect(tokens(policy["script-src"])).not.toContain("'unsafe-eval'")
        expect(tokens(policy["connect-src"])).toEqual(expect.arrayContaining(["'self'", "ipc:", "http://ipc.localhost"]))
        expect(tokens(policy["connect-src"])).not.toContain("https:")
        expect(policy["worker-src"]).toBe("'self'")
    })

    it("preserves local, remote, SVG and Kitty images and runtime-injected styles", () => {
        expect(tokens(policy["img-src"])).toEqual([
            "'self'", "asset:", "http://asset.localhost", "blob:", "data:", "http:", "https:"
        ])
        // DOMPurify also permits raw Markdown audio/video/source elements.
        expect(policy["media-src"]).toBe(policy["img-src"])
        // Vite inlines the small Hanken Grotesk / JetBrains Mono Cyrillic-ext fonts.
        expect(policy["font-src"]).toBe("'self' data:")
        expect(policy["style-src"]).toBe("'self' 'unsafe-inline'")
        // Tauri's style nonces would override unsafe-inline and block dynamic CSS.
        // Script hashing must remain enabled, including the startup theme script.
        expect(security.dangerousDisableAssetCspModification).toEqual(["style-src"])
    })

    it("blocks embedded documents, plugins, base URL changes and form submission", () => {
        for (const directive of ["object-src", "frame-src", "base-uri", "form-action"] as const) {
            expect(policy[directive]).toBe("'none'")
        }
        // The Browser uses a separate native child webview in Tauri, not an iframe.
        expect(JSON.stringify(policy)).not.toContain("yuzora-preview")
    })
})

it("keeps dev server and HMR allowances out of the shipped policy", () => {
    expect(security.csp["script-src"]).toBe("'self'")
    expect(security.csp["connect-src"]).toBe("'self' ipc: http://ipc.localhost")
    expect(tokens(security.devCsp["script-src"])).toContain("http://*:1420")
    // Port-restricted hosts also cover TAURI_DEV_HOST without a fixed LAN address.
    expect(tokens(security.devCsp["connect-src"])).toEqual([
        "'self'", "ipc:", "http://ipc.localhost", "http://*:1420", "ws://*:1420", "ws://*:1421"
    ])
})

it.each(["\n", "\r\n", "\r"])("pins dev inline scripts with %j checkout newlines", (newline) => {
    const html = readFileSync(resolve(import.meta.dirname, "./index.html"), "utf8")
    const inlineScripts = [...html.matchAll(/<script>([\s\S]*?)<\/script>/g)].map((match) => match[1])
    expect(inlineScripts).toHaveLength(1)
    const preamble = react.preambleCode.replace("__BASE__", "/")
    expect(tokens(security.devCsp["script-src"])).toEqual([
        "'self'", "http://*:1420", ...inlineScripts.map(script => hash(script.replace(/\r\n?|\n/g, newline))), hash(preamble)
    ])
})

import { afterEach, describe, expect, it, vi } from "vitest"
import { EditorState } from "@codemirror/state"
import { EditorView } from "@codemirror/view"
import type { LSPClient } from "@codemirror/lsp-client"

import {
    decodeSemanticTokens,
    semanticTokensExtension,
    type DecodedToken,
    type SemanticTokensLegend
} from "./semanticTokens"

describe("decodeSemanticTokens", () => {
    // Legend covering the token types / modifiers used across the delta fixtures.
    const legend: SemanticTokensLegend = {
        tokenTypes: ["keyword", "variable", "function", "parameter"],
        tokenModifiers: ["declaration", "readonly"]
    }

    it("decodes a multi-token delta stream with same-line accumulation and newline reset", () => {
        // 4 tokens, 5 ints each: [deltaLine, deltaStartChar, length, tokenType, tokenModifiers]
        const data = [
            0, 0, 5, 0, 0, // line0 char0 len5  keyword
            0, 6, 1, 1, 0, // same line, char 0+6=6, len1  variable   (char accumulates)
            1, 2, 3, 2, 0b01, // deltaLine1 -> line1, char reset to 2, len3  function [declaration]
            0, 4, 4, 3, 0b11 // same line1, char 2+4=6, len4  parameter [declaration, readonly]
        ]
        const tokens = decodeSemanticTokens(data, legend)
        expect(tokens).toEqual<DecodedToken[]>([
            { line: 0, char: 0, length: 5, type: "keyword", modifiers: [] },
            { line: 0, char: 6, length: 1, type: "variable", modifiers: [] },
            { line: 1, char: 2, length: 3, type: "function", modifiers: ["declaration"] },
            { line: 1, char: 6, length: 4, type: "parameter", modifiers: ["declaration", "readonly"] }
        ])
    })

    it("safely ignores a token whose tokenType index is out of legend range (no throw)", () => {
        const data = [0, 0, 5, 9, 0] // tokenType 9 not in legend
        expect(() => decodeSemanticTokens(data, legend)).not.toThrow()
        expect(decodeSemanticTokens(data, legend)).toEqual([])
    })

    it("does not throw on a legend with non-standard type names", () => {
        const oddLegend: SemanticTokensLegend = { tokenTypes: ["notARealType"], tokenModifiers: [] }
        const data = [0, 0, 3, 0, 0]
        expect(() => decodeSemanticTokens(data, oddLegend)).not.toThrow()
        expect(decodeSemanticTokens(data, oddLegend)[0].type).toBe("notARealType")
    })
})

describe("semanticTokensExtension", () => {
    const views: EditorView[] = []
    afterEach(() => {
        for (const v of views) v.destroy()
        views.length = 0
    })

    function makeView(client: LSPClient, doc: string): EditorView {
        const state = EditorState.create({
            doc,
            extensions: [semanticTokensExtension(client, "file:///x.ts")]
        })
        const view = new EditorView({ state, parent: document.body })
        views.push(view)
        return view
    }

    // Lets the plugin's initial semanticTokens request promise resolve and its
    // dispatch flush into the DOM.
    const flush = () => new Promise((r) => setTimeout(r, 0))

    it("renders cm-st-* marks for a client advertising semanticTokensProvider", async () => {
        const legend = { tokenTypes: ["keyword", "variable"], tokenModifiers: [] }
        const client = {
            serverCapabilities: { semanticTokensProvider: { legend } },
            request: () =>
                Promise.resolve({
                    data: [
                        0, 0, 5, 0, 0, // "const" keyword
                        0, 6, 1, 1, 0 // "x" variable
                    ]
                })
        } as unknown as LSPClient

        const view = makeView(client, "const x = 1")
        await flush()

        expect(view.dom.querySelectorAll(".cm-st-keyword").length).toBeGreaterThan(0)
        expect(view.dom.querySelectorAll(".cm-st-variable").length).toBeGreaterThan(0)
    })

    it("produces an empty extension (no marks) when the server lacks semanticTokensProvider", async () => {
        const client = {
            serverCapabilities: { hoverProvider: true },
            request: () => Promise.resolve({ data: [0, 0, 5, 0, 0] })
        } as unknown as LSPClient

        expect(semanticTokensExtension(client, "file:///x.ts")).toEqual([])

        const view = makeView(client, "const x = 1")
        await flush()
        expect(view.dom.querySelectorAll("[class*='cm-st-']").length).toBe(0)
    })

    it("safely ignores tokens whose type name is non-standard (no mark, no throw)", async () => {
        const legend = { tokenTypes: ["notARealType"], tokenModifiers: [] }
        const client = {
            serverCapabilities: { semanticTokensProvider: { legend } },
            request: () => Promise.resolve({ data: [0, 0, 3, 0, 0] })
        } as unknown as LSPClient

        const view = makeView(client, "abc def")
        await flush()
        expect(view.dom.querySelectorAll("[class*='cm-st-']").length).toBe(0)
    })

    it("discards a stale in-flight fetch after the document changed (generation guard)", async () => {
        vi.useFakeTimers()
        try {
            const legend = { tokenTypes: ["keyword"], tokenModifiers: [] }
            let resolveFirst!: (v: unknown) => void
            let call = 0
            const client = {
                serverCapabilities: { semanticTokensProvider: { legend } },
                request: () => {
                    call++
                    // First (mount) request is held open so it resolves *after* the edit.
                    if (call === 1) return new Promise((r) => (resolveFirst = r))
                    return Promise.resolve({ data: [] })
                }
            } as unknown as LSPClient

            const view = makeView(client, "const x = 1")
            // Edit bumps the generation and schedules a fresh (debounced) fetch.
            view.dispatch({ changes: { from: 0, insert: "y" } })

            // The now-stale mount fetch resolves late with keyword tokens.
            resolveFirst({ data: [0, 0, 5, 0, 0] })
            for (let i = 0; i < 10; i++) await Promise.resolve()

            // Stale tokens must not win over the current document.
            expect(view.dom.querySelectorAll(".cm-st-keyword").length).toBe(0)
        } finally {
            vi.clearAllTimers()
            vi.useRealTimers()
        }
    })

    it("skips out-of-order tokens without throwing and still renders valid ones", async () => {
        const legend = { tokenTypes: ["keyword", "variable"], tokenModifiers: [] }
        const client = {
            serverCapabilities: { semanticTokensProvider: { legend } },
            request: () =>
                Promise.resolve({
                    data: [
                        0, 5, 3, 0, 0, // keyword at char 5..8
                        0, -4, 2, 1, 0 // variable at char 1..3 -> `from` moves backwards
                    ]
                })
        } as unknown as LSPClient

        const view = makeView(client, "abcdefghij")
        await flush()

        // The reversed token is dropped; the earlier valid one still renders (no throw).
        expect(view.dom.querySelectorAll(".cm-st-keyword").length).toBeGreaterThan(0)
    })
})

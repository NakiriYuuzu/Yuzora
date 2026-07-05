import { it, expect, vi } from "vitest"

import { requestDocumentSymbols, requestWorkspaceSymbols } from "./symbols"

const range = (line: number) => ({
    start: { line, character: 0 },
    end: { line, character: 5 }
})

const ZERO_RANGE = { start: { line: 0, character: 0 }, end: { line: 0, character: 0 } }

it("requestDocumentSymbols sends textDocument/documentSymbol and flattens DocumentSymbol[] children in outline order", async () => {
    const tree = [
        {
            name: "Cls",
            kind: 5,
            detail: "class",
            range: range(0),
            selectionRange: range(1),
            children: [
                { name: "method", kind: 6, range: range(2), selectionRange: range(3) }
            ]
        }
    ]
    const client = { request: vi.fn(async () => tree), sync: vi.fn() }

    const out = await requestDocumentSymbols(client as never, "file:///a.ts")

    const call = client.request.mock.calls[0] as unknown[]
    expect(call[0]).toBe("textDocument/documentSymbol")
    expect(call[1]).toEqual({ textDocument: { uri: "file:///a.ts" } })
    // selectionRange (the name location) is used for the flat range — parent
    // before child (depth-first pre-order = outline order).
    expect(out).toEqual([
        { name: "Cls", kind: 5, detail: "class", range: range(1) },
        { name: "method", kind: 6, range: range(3) }
    ])
})

it("requestDocumentSymbols flattens SymbolInformation[] (location form)", async () => {
    const flat = [
        { name: "func", kind: 12, location: { uri: "file:///a.ts", range: range(7) } }
    ]
    const client = { request: vi.fn(async () => flat), sync: vi.fn() }

    const out = await requestDocumentSymbols(client as never, "file:///a.ts")

    expect(out).toEqual([{ name: "func", kind: 12, range: range(7) }])
})

it("requestDocumentSymbols returns [] for a null / non-array response without throwing", async () => {
    const client = { request: vi.fn(async () => null), sync: vi.fn() }
    expect(await requestDocumentSymbols(client as never, "file:///a.ts")).toEqual([])
})

it("requestDocumentSymbols degrades a malformed (truthy, no .start) range to a zero range", async () => {
    // A non-conforming server sends `range: {}` — truthy, so a nullish / `if (range)`
    // guard lets it through; selecting the symbol would then read range.start.line
    // on an empty object and throw. It must map to ZERO_RANGE instead.
    const tree = [{ name: "Bad", kind: 12, range: {} }]
    const client = { request: vi.fn(async () => tree), sync: vi.fn() }

    const out = await requestDocumentSymbols(client as never, "file:///a.ts")

    expect(out).toEqual([{ name: "Bad", kind: 12, range: ZERO_RANGE }])
})

it("requestWorkspaceSymbols sends workspace/symbol with the query and maps uri/range", async () => {
    const items = [
        { name: "Foo", kind: 5, location: { uri: "file:///b.ts", range: range(9) } }
    ]
    const client = { request: vi.fn(async () => items), sync: vi.fn() }

    const out = await requestWorkspaceSymbols(client as never, "Foo")

    const call = client.request.mock.calls[0] as unknown[]
    expect(call[0]).toBe("workspace/symbol")
    expect(call[1]).toEqual({ query: "Foo" })
    expect(out).toEqual([{ name: "Foo", kind: 5, uri: "file:///b.ts", range: range(9) }])
})

it("requestWorkspaceSymbols degrades a malformed (truthy, no .start) range to a zero range", async () => {
    const items = [{ name: "Bad", kind: 5, location: { uri: "file:///b.ts", range: {} } }]
    const client = { request: vi.fn(async () => items), sync: vi.fn() }

    const out = await requestWorkspaceSymbols(client as never, "Bad")

    expect(out).toEqual([{ name: "Bad", kind: 5, uri: "file:///b.ts", range: ZERO_RANGE }])
})

it("requestWorkspaceSymbols maps a WorkspaceSymbol whose location has only a uri to a zero range", async () => {
    // Newer WorkspaceSymbol may omit range (resolved lazily) — selection must still
    // open the file, landing on line 1 (ZERO_RANGE).
    const items = [{ name: "Lazy", kind: 5, location: { uri: "file:///c.ts" } }]
    const client = { request: vi.fn(async () => items), sync: vi.fn() }

    const out = await requestWorkspaceSymbols(client as never, "Lazy")

    expect(out).toEqual([{ name: "Lazy", kind: 5, uri: "file:///c.ts", range: ZERO_RANGE }])
})

it("requestWorkspaceSymbols returns [] on an empty response without throwing", async () => {
    const client = { request: vi.fn(async () => null), sync: vi.fn() }
    expect(await requestWorkspaceSymbols(client as never, "x")).toEqual([])
})

it("requestDocumentSymbols flushes pending edits (sync) before sending the request", async () => {
    // The built-in LSP features sync() before every request so the server answers
    // against the buffer the user sees, not a stale sync point. Assert the order.
    const order: string[] = []
    const client = {
        sync: vi.fn(() => order.push("sync")),
        request: vi.fn(async () => {
            order.push("request")
            return []
        })
    }
    await requestDocumentSymbols(client as never, "file:///a.ts")
    expect(order).toEqual(["sync", "request"])
})

it("requestWorkspaceSymbols flushes pending edits (sync) before sending the request", async () => {
    const order: string[] = []
    const client = {
        sync: vi.fn(() => order.push("sync")),
        request: vi.fn(async () => {
            order.push("request")
            return []
        })
    }
    await requestWorkspaceSymbols(client as never, "q")
    expect(order).toEqual(["sync", "request"])
})

it("requestWorkspaceSymbols skips a record whose name is not a string", async () => {
    // A non-conforming server may omit name; the picker renders item.name, so a
    // nameless record must be dropped rather than surfaced as `undefined`.
    const items = [
        { kind: 5, location: { uri: "file:///b.ts", range: range(1) } },
        { name: "Ok", kind: 5, location: { uri: "file:///c.ts", range: range(2) } }
    ]
    const client = { request: vi.fn(async () => items), sync: vi.fn() }

    const out = await requestWorkspaceSymbols(client as never, "x")

    expect(out).toEqual([{ name: "Ok", kind: 5, uri: "file:///c.ts", range: range(2) }])
})

it("requestDocumentSymbols degrades a flat SymbolInformation with a malformed location.range to a zero range", async () => {
    // SymbolInformation[] (flat, detected by `location`) with a truthy but empty
    // range — selecting it would read range.start.line and throw unless the
    // location branch validates the range shape. Must map to ZERO_RANGE.
    const flat = [{ name: "Flat", kind: 12, location: { uri: "file:///a.ts", range: {} } }]
    const client = { request: vi.fn(async () => flat), sync: vi.fn() }

    const out = await requestDocumentSymbols(client as never, "file:///a.ts")

    expect(out).toEqual([{ name: "Flat", kind: 12, range: ZERO_RANGE }])
})

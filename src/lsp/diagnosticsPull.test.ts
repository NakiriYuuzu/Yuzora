import { it, expect, vi, afterEach } from "vitest"
import { EditorState } from "@codemirror/state"
import { EditorView } from "@codemirror/view"
import { forEachDiagnostic } from "@codemirror/lint"

import { pullDiagnostics, diagnosticsPullExtension } from "./diagnosticsPull"
import type { LspDiagnostic } from "./diagnosticsPull"

const flush = async () => {
    for (let i = 0; i < 5; i++) await Promise.resolve()
}

const item = (message: string): LspDiagnostic => ({
    range: { start: { line: 0, character: 0 }, end: { line: 0, character: 1 } },
    message,
    severity: 1
})

const messagesIn = (view: EditorView): string[] => {
    const out: string[] = []
    forEachDiagnostic(view.state, (d) => out.push(d.message))
    return out
}

afterEach(() => {
    vi.useRealTimers()
    vi.restoreAllMocks()
})

function fakeClient(serverCapabilities: unknown, requestImpl?: (...a: unknown[]) => unknown) {
    return {
        serverCapabilities,
        request: vi.fn(requestImpl ?? (async () => ({ kind: "full", items: [] })))
    }
}

it("pullDiagnostics sends textDocument/diagnostic and returns the full report items", async () => {
    const items: LspDiagnostic[] = [
        { range: { start: { line: 0, character: 0 }, end: { line: 0, character: 3 } }, message: "bad", severity: 1 }
    ]
    const client = fakeClient({ diagnosticProvider: {} }, async () => ({ kind: "full", items }))
    const out = await pullDiagnostics(client as never, "file:///a.ts")

    expect(client.request).toHaveBeenCalledWith("textDocument/diagnostic", { textDocument: { uri: "file:///a.ts" } })
    expect(out).toEqual(items)
})

it("pullDiagnostics returns null for an unchanged report (signals: keep prior diagnostics)", async () => {
    const client = fakeClient({ diagnosticProvider: {} }, async () => ({ kind: "unchanged", resultId: "r1" }))
    expect(await pullDiagnostics(client as never, "file:///a.ts")).toBeNull()
})

it("pullDiagnostics returns [] for an empty full report (signals: clear diagnostics)", async () => {
    const client = fakeClient({ diagnosticProvider: {} }, async () => ({ kind: "full", items: [] }))
    expect(await pullDiagnostics(client as never, "file:///a.ts")).toEqual([])
})

it("diagnosticsPullExtension is inert (sends nothing) when the server has no diagnosticProvider", () => {
    const client = fakeClient({})
    const ext = diagnosticsPullExtension(client as never, "file:///a.ts")
    const view = new EditorView({ state: EditorState.create({ doc: "x", extensions: [ext] }) })
    view.dispatch({ changes: { from: 1, insert: "y" } })

    expect(client.request).not.toHaveBeenCalled()
    view.destroy()
})

it("diagnosticsPullExtension pulls once on mount and coalesces docChanged pulls via a debounce", () => {
    vi.useFakeTimers()
    const client = fakeClient({ diagnosticProvider: {} })
    const ext = diagnosticsPullExtension(client as never, "file:///a.ts")
    const view = new EditorView({ state: EditorState.create({ doc: "x", extensions: [ext] }) })

    expect(client.request).toHaveBeenCalledTimes(1) // initial mount pull

    view.dispatch({ changes: { from: 1, insert: "y" } })
    view.dispatch({ changes: { from: 2, insert: "z" } })
    expect(client.request).toHaveBeenCalledTimes(1) // still within the debounce window

    vi.advanceTimersByTime(500)
    expect(client.request).toHaveBeenCalledTimes(2) // two edits coalesced into one pull

    view.destroy()
})

it("drops a stale in-flight pull whose document changed mid-request (generation guard, no stale-wins)", async () => {
    vi.useFakeTimers()
    const resolvers: Array<() => void> = []
    const payloads = ["mount", "afterEdit"]
    let call = 0
    const request = vi.fn(
        () =>
            new Promise((resolve) => {
                const i = call++
                resolvers.push(() => resolve({ kind: "full", items: [item(payloads[i])] }))
            })
    )
    const client = { serverCapabilities: { diagnosticProvider: {} }, request }
    const ext = diagnosticsPullExtension(client as never, "file:///a.ts")
    const view = new EditorView({ state: EditorState.create({ doc: "xxxx", extensions: [ext] }) })

    // mount pull is call 0 (generation 0), still pending. A doc change bumps the
    // generation to 1 and schedules a fresh pull.
    view.dispatch({ changes: { from: 1, insert: "y" } })
    vi.advanceTimersByTime(500) // debounced run -> call 1 (generation 1), pending

    // The newer pull resolves first and renders.
    resolvers[1]()
    await flush()
    expect(messagesIn(view)).toEqual(["afterEdit"])

    // The stale mount pull resolves late; its generation (0) no longer matches,
    // so it must NOT overwrite the newer diagnostics.
    resolvers[0]()
    await flush()
    expect(messagesIn(view)).toEqual(["afterEdit"])

    view.destroy()
})

it("an unchanged pull keeps the previously rendered diagnostics (does not clear them)", async () => {
    vi.useFakeTimers()
    let call = 0
    const request = vi.fn(async () =>
        call++ === 0 ? { kind: "full", items: [item("keep")] } : { kind: "unchanged", resultId: "r1" }
    )
    const client = { serverCapabilities: { diagnosticProvider: {} }, request }
    const ext = diagnosticsPullExtension(client as never, "file:///a.ts")
    const view = new EditorView({ state: EditorState.create({ doc: "xxxx", extensions: [ext] }) })

    await flush() // mount pull renders [keep]
    expect(messagesIn(view)).toEqual(["keep"])

    view.dispatch({ changes: { from: 1, insert: "y" } })
    vi.advanceTimersByTime(500) // debounced pull -> unchanged
    await flush()
    expect(messagesIn(view)).toEqual(["keep"]) // not cleared

    view.destroy()
})

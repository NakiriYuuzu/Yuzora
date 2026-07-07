import { it, expect, vi, afterEach } from "vitest"
import { EditorState } from "@codemirror/state"
import { EditorView } from "@codemirror/view"

import { setDiagnostics } from "@codemirror/lint"

import { codeActionsFor, applyWorkspaceEdit, lspLintSource } from "./codeActions"
import type { WorkspaceEdit } from "./renameCompat"
import type { LspDiagnostic } from "./diagnosticsPull"
import i18n from "../lib/i18n"

const flush = async () => {
    for (let i = 0; i < 5; i++) await Promise.resolve()
}

afterEach(() => {
    vi.restoreAllMocks()
})

const diag = (start: number, end: number, message: string, severity = 1): LspDiagnostic => ({
    range: { start: { line: 0, character: start }, end: { line: 0, character: end } },
    message,
    severity
})

it("codeActionsFor requests textDocument/codeAction with context.diagnostics carrying the message (vtsls rejects without it)", async () => {
    const client = { sync: vi.fn(), request: vi.fn(async () => [{ title: "Fix", edit: { changes: {} } }]) }
    const d = diag(4, 7, "'foo' is declared but never used")

    await codeActionsFor(client as never, "file:///a.ts", d)

    const call = client.request.mock.calls[0] as unknown[]
    const method = call[0] as string
    const params = call[1] as {
        textDocument: unknown
        range: unknown
        context: { diagnostics: Array<{ message: string }>; only?: string[] }
    }
    expect(method).toBe("textDocument/codeAction")
    expect(params.textDocument).toEqual({ uri: "file:///a.ts" })
    expect(params.range).toEqual(d.range)
    expect(params.context.diagnostics[0].message).toBe("'foo' is declared but never used")
    expect(params.context.only).toEqual(["quickfix"]) // narrow to quick fixes (C6)
})

it("codeActionsFor maps returned actions preserving title, kind and edit", async () => {
    const edit: WorkspaceEdit = { changes: { "file:///a.ts": [] } }
    const client = { sync: vi.fn(), request: vi.fn(async () => [{ title: "Remove unused", kind: "quickfix", edit }]) }

    const out = await codeActionsFor(client as never, "file:///a.ts", diag(0, 0, "m"))
    expect(out).toEqual([{ title: "Remove unused", kind: "quickfix", edit }])
})

it("codeActionsFor synchronizes pending changes (client.sync) before requesting, so the fix targets the current doc (R3-3)", async () => {
    const order: string[] = []
    const client = {
        sync: vi.fn(() => void order.push("sync")),
        request: vi.fn(async () => {
            order.push("request")
            return []
        })
    }

    await codeActionsFor(client as never, "file:///a.ts", diag(0, 0, "m"))

    expect(client.sync).toHaveBeenCalled()
    expect(order).toEqual(["sync", "request"])
})

it("applyWorkspaceEdit applies a changes-form edit to the real view document", () => {
    const view = new EditorView({ state: EditorState.create({ doc: "let foo = 1\n" }) })
    const uri = "file:///a.ts"
    const edit: WorkspaceEdit = {
        changes: {
            [uri]: [{ range: { start: { line: 0, character: 4 }, end: { line: 0, character: 7 } }, newText: "bar" }]
        }
    }

    applyWorkspaceEdit(view, edit, uri)
    expect(view.state.doc.toString()).toBe("let bar = 1\n")
    view.destroy()
})

it("applyWorkspaceEdit applies a documentChanges-form edit (rust-analyzer) to the real view document", () => {
    const view = new EditorView({ state: EditorState.create({ doc: "let foo = 1\n" }) })
    const uri = "file:///a.ts"
    const edit: WorkspaceEdit = {
        documentChanges: [
            {
                textDocument: { uri, version: 3 },
                edits: [{ range: { start: { line: 0, character: 4 }, end: { line: 0, character: 7 } }, newText: "bar" }]
            }
        ]
    }

    applyWorkspaceEdit(view, edit, uri)
    expect(view.state.doc.toString()).toBe("let bar = 1\n")
    view.destroy()
})

it("applyWorkspaceEdit applies every edit when documentChanges holds multiple entries for the same uri (C3)", () => {
    const view = new EditorView({ state: EditorState.create({ doc: "let foo = 1\nlet bar = 2\n" }) })
    const uri = "file:///a.ts"
    const edit: WorkspaceEdit = {
        documentChanges: [
            {
                textDocument: { uri, version: 1 },
                edits: [{ range: { start: { line: 0, character: 4 }, end: { line: 0, character: 7 } }, newText: "AAA" }]
            },
            {
                textDocument: { uri, version: 1 },
                edits: [{ range: { start: { line: 1, character: 4 }, end: { line: 1, character: 7 } }, newText: "BBB" }]
            }
        ]
    }

    applyWorkspaceEdit(view, edit, uri)
    expect(view.state.doc.toString()).toBe("let AAA = 1\nlet BBB = 2\n")
    view.destroy()
})

it("applyWorkspaceEdit drops the whole edit (atomic) when any range is inverted, leaving the doc untouched (R2-3)", () => {
    const view = new EditorView({ state: EditorState.create({ doc: "let foo = 1\nlet bar = 2\n" }) })
    const uri = "file:///a.ts"
    const edit: WorkspaceEdit = {
        changes: {
            [uri]: [
                { range: { start: { line: 0, character: 4 }, end: { line: 0, character: 7 } }, newText: "AAA" },
                // Inverted: start (line 2) clamps to a larger offset than end (line 0).
                { range: { start: { line: 2, character: 0 }, end: { line: 0, character: 0 } }, newText: "X" }
            ]
        }
    }

    expect(() => applyWorkspaceEdit(view, edit, uri)).not.toThrow()
    // Atomic: the valid AAA edit is dropped too rather than half-applying a
    // semantically broken WorkspaceEdit.
    expect(view.state.doc.toString()).toBe("let foo = 1\nlet bar = 2\n")
    view.destroy()
})

it("lspLintSource yields diagnostics whose quick-fix action runs the full apply chain (C9/C11)", async () => {
    const uri = "file:///a.ts"
    const fixEdit: WorkspaceEdit = {
        changes: {
            [uri]: [{ range: { start: { line: 0, character: 4 }, end: { line: 0, character: 7 } }, newText: "bar" }]
        }
    }
    const client = {
        serverCapabilities: { diagnosticProvider: {} },
        sync: vi.fn(),
        request: vi.fn(async (method: string) => {
            if (method === "textDocument/diagnostic") {
                return {
                    kind: "full",
                    items: [
                        {
                            range: { start: { line: 0, character: 4 }, end: { line: 0, character: 7 } },
                            message: "unused",
                            severity: 2
                        }
                    ]
                }
            }
            if (method === "textDocument/codeAction") {
                return [{ title: "Remove unused", kind: "quickfix", edit: fixEdit }]
            }
            return []
        })
    }
    const view = new EditorView({ state: EditorState.create({ doc: "let foo = 1\n" }) })

    const source = lspLintSource(client as never, uri)
    const diags = await source(view)

    expect(diags).toHaveLength(1)
    expect(diags[0]).toMatchObject({ from: 4, to: 7, severity: "warning", message: "unused" })
    expect(diags[0].actions).toHaveLength(1)
    expect(diags[0].actions![0].name).toBe(i18n.t("quickFix", { ns: "lsp" })) // localized label (C11)

    // Activating the action drives codeActionsFor -> pick edit -> applyWorkspaceEdit.
    diags[0].actions![0].apply(view, 4, 7)
    await flush()
    expect(view.state.doc.toString()).toBe("let bar = 1\n")
    view.destroy()
})

it("lspLintSource re-emits unchanged diagnostics at their mapped (post-edit) offsets, not stale ones", async () => {
    const client = {
        serverCapabilities: { diagnosticProvider: {} },
        request: vi.fn(async () => ({ kind: "unchanged", resultId: "r1" }))
    }
    const view = new EditorView({ state: EditorState.create({ doc: "let foo = 1\n" }) })
    // Seed one already-rendered diagnostic, then edit the doc ABOVE it so the lint
    // framework maps the decoration forward while the Diagnostic object keeps its
    // original from/to. An unchanged pull must return the MAPPED offsets (via
    // forEachDiagnostic's 2nd/3rd args), not the object's stale ones.
    view.dispatch(setDiagnostics(view.state, [{ from: 4, to: 7, severity: "warning", message: "prior" }]))
    const insert = "// added\n"
    view.dispatch({ changes: { from: 0, insert } })

    const source = lspLintSource(client as never, "file:///a.ts")
    const diags = await source(view)

    expect(diags.map((d) => d.message)).toEqual(["prior"])
    expect(diags[0].from).toBe(4 + insert.length)
    expect(diags[0].to).toBe(7 + insert.length)
    view.destroy()
})

it("lspLintSource keeps the prior diagnostics when a pull throws (transient error, not a clear)", async () => {
    const client = {
        serverCapabilities: { diagnosticProvider: {} },
        request: vi.fn(async () => {
            throw new Error("transient")
        })
    }
    const view = new EditorView({ state: EditorState.create({ doc: "let foo = 1\n" }) })
    view.dispatch(setDiagnostics(view.state, [{ from: 4, to: 7, severity: "warning", message: "prior" }]))

    const source = lspLintSource(client as never, "file:///a.ts")
    const diags = await source(view)

    expect(diags.map((d) => d.message)).toEqual(["prior"])
    view.destroy()
})

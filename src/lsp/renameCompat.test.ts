import { it, expect } from "vitest"

import { workspaceEditToChanges } from "./renameCompat"
import type { TextEdit, WorkspaceEdit } from "./renameCompat"

const te = (start: number, end: number, newText: string): TextEdit => ({
    range: { start: { line: 0, character: start }, end: { line: 0, character: end } },
    newText
})

it("normalizes the changes-form WorkspaceEdit (vtsls) into a uri/edits list", () => {
    const edit: WorkspaceEdit = { changes: { "file:///a.ts": [te(0, 3, "foo")] } }
    expect(workspaceEditToChanges(edit)).toEqual([{ uri: "file:///a.ts", edits: [te(0, 3, "foo")] }])
})

it("normalizes the documentChanges-form WorkspaceEdit (rust-analyzer, versioned) into the same shape", () => {
    const edit: WorkspaceEdit = {
        documentChanges: [{ textDocument: { uri: "file:///a.ts", version: 7 }, edits: [te(0, 3, "foo")] }]
    }
    expect(workspaceEditToChanges(edit)).toEqual([{ uri: "file:///a.ts", edits: [te(0, 3, "foo")] }])
})

it("prefers documentChanges over changes when both are present (LSP precedence)", () => {
    const edit: WorkspaceEdit = {
        changes: { "file:///stale.ts": [te(0, 1, "x")] },
        documentChanges: [{ textDocument: { uri: "file:///a.ts", version: 1 }, edits: [te(0, 3, "foo")] }]
    }
    expect(workspaceEditToChanges(edit)).toEqual([{ uri: "file:///a.ts", edits: [te(0, 3, "foo")] }])
})

it("returns an empty list for an edit carrying neither field", () => {
    expect(workspaceEditToChanges({})).toEqual([])
})

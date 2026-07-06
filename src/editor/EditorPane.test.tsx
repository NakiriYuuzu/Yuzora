import { afterEach, describe, expect, it, vi } from "vitest"
import { act, cleanup, render, waitFor } from "@testing-library/react"
import type { EditorView } from "@codemirror/view"

import { useWorkspaceStore } from "../state/workspaceStore"

// Spy on the view registry so we can assert the exact (path, view) call args.
const registerView = vi.fn()
const unregisterView = vi.fn()
vi.mock("./viewRegistry", () => ({
    registerView: (path: string, view: EditorView) => registerView(path, view),
    unregisterView: (path: string, view?: EditorView) => unregisterView(path, view),
    getView: vi.fn()
}))

vi.mock("./documentRegistry", () => ({
    getDocument: vi.fn(async () => ({
        result: { kind: "full", content: "one\ntwo\nthree", size: 13 }
    })),
    updateBuffer: vi.fn(),
    documentGeneration: vi.fn(() => 0)
}))

vi.mock("../lib/ipc", () => ({
    saveFile: vi.fn(async () => 0)
}))

vi.mock("@/features/logs/userAction", () => ({
    logUserAction: vi.fn(async () => undefined)
}))

vi.mock("../workbench/ExternalChangeResolver", () => ({
    maybeInterceptSave: () => false
}))

const { EditorPane } = await import("./EditorPane")

const PATH = "/w/a.ts"

afterEach(() => {
    cleanup()
    vi.clearAllMocks()
    useWorkspaceStore.setState({ pendingReveal: null })
})

describe("EditorPane", () => {
    it("unregisters its own view (path, view) on unmount (m4)", async () => {
        useWorkspaceStore.setState({ pendingReveal: null })
        const { unmount } = render(<EditorPane path={PATH} />)
        await waitFor(() => expect(registerView).toHaveBeenCalled())
        const view = registerView.mock.calls[0][1]
        unmount()
        expect(unregisterView).toHaveBeenCalledWith(PATH, view)
    })

    it("revealLine clamps below 1 without throwing (T18)", async () => {
        useWorkspaceStore.setState({ pendingReveal: null })
        render(<EditorPane path={PATH} />)
        await waitFor(() => expect(registerView).toHaveBeenCalled())
        // Requesting a reveal at line 0 must not throw — it is clamped to line 1.
        expect(() =>
            act(() => {
                useWorkspaceStore.getState().requestReveal(PATH, 0)
            })
        ).not.toThrow()
    })
})

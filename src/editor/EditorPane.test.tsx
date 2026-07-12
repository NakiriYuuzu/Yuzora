import { afterEach, describe, expect, it, vi } from "vitest"
import { act, cleanup, fireEvent, render, waitFor } from "@testing-library/react"
import type { EditorView } from "@codemirror/view"

import { useWorkspaceStore } from "../state/workspaceStore"
import { useEditorSettingsStore } from "../state/editorSettingsStore"
import { useContextMenuStore } from "../state/contextMenuStore"

// Spy on the view registry so we can assert the exact (path, view) call args.
const registerView = vi.fn()
const unregisterView = vi.fn()
const updateViewMetadata = vi.fn()
vi.mock("./viewRegistry", () => ({
    registerView: (path: string, view: EditorView, metadata: unknown) => registerView(path, view, metadata),
    unregisterView: (path: string, view?: EditorView) => unregisterView(path, view),
    updateViewMetadata: (path: string, view: EditorView, metadata: unknown) =>
        updateViewMetadata(path, view, metadata),
    getView: vi.fn(),
    getViewEntry: vi.fn()
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
    useContextMenuStore.setState({ request: null, x: 0, y: 0, availabilityRevision: 0 })
    useEditorSettingsStore.setState({ fontSize: 13, minimap: false })
})

describe("EditorPane", () => {
    it("right-click focuses and targets the clicked pane group", async () => {
        useWorkspaceStore.setState({
            workspacePath: "/w",
            activeGroupIndex: 0,
            groups: [
                { tabs: [], activePath: null },
                {
                    tabs: [{ path: PATH, name: "a.ts", dirty: false, externallyModified: false }],
                    activePath: PATH
                }
            ]
        })
        const { container } = render(<EditorPane path={PATH} groupIndex={1} />)
        await waitFor(() => expect(registerView).toHaveBeenCalled())

        fireEvent.contextMenu(container.querySelector(".editor-pane")!)

        expect(useWorkspaceStore.getState().activeGroupIndex).toBe(1)
        expect(useContextMenuStore.getState().request).toEqual({
            kind: "editor",
            workspacePath: "/w",
            path: PATH,
            groupIndex: 1
        })
    })

    it("unregisters its own view (path, view) on unmount (m4)", async () => {
        useWorkspaceStore.setState({ pendingReveal: null })
        const { unmount } = render(<EditorPane path={PATH} groupIndex={0} />)
        await waitFor(() => expect(registerView).toHaveBeenCalled())
        const view = registerView.mock.calls[0][1]
        unmount()
        expect(unregisterView).toHaveBeenCalledWith(PATH, view)
    })

    it("revealLine clamps below 1 without throwing (T18)", async () => {
        useWorkspaceStore.setState({ pendingReveal: null })
        render(<EditorPane path={PATH} groupIndex={0} />)
        await waitFor(() => expect(registerView).toHaveBeenCalled())
        // Requesting a reveal at line 0 must not throw — it is clamped to line 1.
        expect(() =>
            act(() => {
                useWorkspaceStore.getState().requestReveal(PATH, 0)
            })
        ).not.toThrow()
    })

    it("applies the editorSettings font size as a CSS variable on the view root at creation (F6)", async () => {
        useEditorSettingsStore.setState({ fontSize: 15, minimap: false })
        render(<EditorPane path={PATH} groupIndex={0} />)
        await waitFor(() => expect(registerView).toHaveBeenCalled())
        const view = registerView.mock.calls[0][1] as EditorView
        expect(view.dom.style.getPropertyValue("--yz-editor-font-size")).toBe("15px")
    })

    it("changing the font size live updates the open view's CSS variable (F6)", async () => {
        render(<EditorPane path={PATH} groupIndex={0} />)
        await waitFor(() => expect(registerView).toHaveBeenCalled())
        const view = registerView.mock.calls[0][1] as EditorView
        expect(view.dom.style.getPropertyValue("--yz-editor-font-size")).toBe("13px")
        act(() => useEditorSettingsStore.getState().setFontSize(14))
        expect(view.dom.style.getPropertyValue("--yz-editor-font-size")).toBe("14px")
    })

    it("mounts the minimap strip when the setting is on and toggles it live via the compartment (F6)", async () => {
        useEditorSettingsStore.setState({ fontSize: 13, minimap: true })
        render(<EditorPane path={PATH} groupIndex={0} />)
        await waitFor(() => expect(registerView).toHaveBeenCalled())
        const view = registerView.mock.calls[0][1] as EditorView
        // Built with minimap on: one bar per line of the mocked "one\ntwo\nthree".
        expect(view.dom.querySelectorAll(".yz-minimap-bar").length).toBe(3)
        act(() => useEditorSettingsStore.getState().setMinimap(false))
        expect(view.dom.querySelector(".yz-minimap")).toBeNull()
        act(() => useEditorSettingsStore.getState().setMinimap(true))
        expect(view.dom.querySelector(".yz-minimap")).not.toBeNull()
    })
})

import { afterEach, describe, expect, it, vi } from "vitest"
import { act, cleanup, fireEvent, render, waitFor } from "@testing-library/react"
import type { EditorView } from "@codemirror/view"

import { useWorkspaceStore } from "../state/workspaceStore"
import { useEditorSettingsStore } from "../state/editorSettingsStore"
import { useContextMenuStore } from "../state/contextMenuStore"
import type { OpenFileResult } from "../lib/types"

// Spy on the view registry so we can assert the exact (path, view) call args.
const registerView = vi.fn()
const unregisterView = vi.fn()
const updateViewMetadata = vi.fn()
const getDocument = vi.fn(async (): Promise<{ result: OpenFileResult }> => ({
    result: { kind: "full", content: "one\ntwo\nthree", size: 13, lineEnding: "lf" }
}))
const documentGeneration = vi.fn(() => 0)
const saveFile = vi.fn(async () => 0)
vi.mock("./viewRegistry", () => ({
    registerView: (path: string, view: EditorView, metadata: unknown) => registerView(path, view, metadata),
    unregisterView: (path: string, view?: EditorView) => unregisterView(path, view),
    updateViewMetadata: (path: string, view: EditorView, metadata: unknown) =>
        updateViewMetadata(path, view, metadata),
    getView: vi.fn(),
    getViewEntry: vi.fn()
}))

vi.mock("./documentRegistry", () => ({
    getDocument: () => getDocument(),
    updateBuffer: vi.fn(),
    documentGeneration: () => documentGeneration()
}))

vi.mock("../lib/ipc", () => ({
    saveFile: () => saveFile()
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
    getDocument.mockResolvedValue({
        result: { kind: "full", content: "one\ntwo\nthree", size: 13, lineEnding: "lf" }
    })
    documentGeneration.mockReturnValue(0)
    useWorkspaceStore.setState({ pendingReveal: null })
    useContextMenuStore.setState({ request: null, x: 0, y: 0, availabilityRevision: 0 })
    useEditorSettingsStore.setState({ fontSize: 13, minimap: false })
})

describe("EditorPane", () => {
    it("hydrates the editable tab line ending without marking it dirty", async () => {
        useWorkspaceStore.setState({
            workspacePath: "/w",
            activeGroupIndex: 0,
            groups: [{
                activePath: PATH,
                tabs: [{ path: PATH, name: "a.ts", dirty: false, externallyModified: false }]
            }]
        })

        render(<EditorPane path={PATH} groupIndex={0} />)

        await waitFor(() =>
            expect(useWorkspaceStore.getState().groups[0].tabs[0]).toMatchObject({
                lineEnding: "lf",
                dirty: false
            })
        )
    })

    it("preserves an explicit target across an ordinary pane remount", async () => {
        useWorkspaceStore.setState({
            workspacePath: "/w",
            activeGroupIndex: 0,
            groups: [{
                activePath: PATH,
                tabs: [{ path: PATH, name: "a.ts", dirty: false, externallyModified: false }]
            }]
        })

        const first = render(<EditorPane path={PATH} groupIndex={0} />)
        await waitFor(() =>
            expect(useWorkspaceStore.getState().getLineEnding(PATH)).toBe("lf")
        )
        act(() => useWorkspaceStore.getState().setLineEnding(PATH, "crlf"))
        first.unmount()

        render(<EditorPane path={PATH} groupIndex={0} />)
        await waitFor(() => expect(registerView).toHaveBeenCalled())

        expect(useWorkspaceStore.getState().groups[0].tabs[0]).toMatchObject({
            lineEnding: "crlf",
            lineEndingGeneration: 0,
            dirty: true
        })
    })

    it("replaces the target when a successful disk reload advances generation", async () => {
        useWorkspaceStore.setState({
            workspacePath: "/w",
            activeGroupIndex: 0,
            groups: [{
                activePath: PATH,
                tabs: [{
                    path: PATH,
                    name: "a.ts",
                    dirty: true,
                    externallyModified: true,
                    lineEnding: "crlf",
                    lineEndingGeneration: 0
                }]
            }]
        })
        getDocument.mockResolvedValue({
            result: { kind: "full", content: "disk\n", size: 5, lineEnding: "lf" }
        })
        documentGeneration.mockReturnValue(1)

        render(<EditorPane path={PATH} groupIndex={0} />)

        await waitFor(() =>
            expect(useWorkspaceStore.getState().groups[0].tabs[0]).toMatchObject({
                lineEnding: "lf",
                lineEndingGeneration: 1
            })
        )
    })

    it.each([
        ["binary", { kind: "binary", size: 8 }],
        ["tooLarge", { kind: "tooLarge", size: 20_000_000 }],
        [
            "nonUtf8Readonly",
            { kind: "nonUtf8Readonly", content: "legacy", encoding: "windows-1252", size: 6 }
        ]
    ] as const)("successful reload to %s clears stale editable metadata", async (_kind, result) => {
        useWorkspaceStore.setState({
            workspacePath: "/w",
            activeGroupIndex: 0,
            groups: [{
                activePath: PATH,
                tabs: [{
                    path: PATH,
                    name: "a.ts",
                    dirty: false,
                    externallyModified: false,
                    lineEnding: "lf",
                    lineEndingGeneration: 0
                }]
            }]
        })
        getDocument.mockResolvedValue({ result })
        documentGeneration.mockReturnValue(1)

        render(<EditorPane path={PATH} groupIndex={0} />)

        await waitFor(() =>
            expect(useWorkspaceStore.getState().groups[0].tabs[0]).toMatchObject({
                lineEnding: undefined,
                lineEndingGeneration: 1
            })
        )
    })

    it("readonly reload keeps Mod-S from calling saveFile after metadata is cleared", async () => {
        useWorkspaceStore.setState({
            workspacePath: "/w",
            activeGroupIndex: 0,
            groups: [{
                activePath: PATH,
                tabs: [{
                    path: PATH,
                    name: "a.ts",
                    dirty: false,
                    externallyModified: false,
                    lineEnding: "lf",
                    lineEndingGeneration: 0
                }]
            }]
        })
        getDocument.mockResolvedValue({
            result: {
                kind: "nonUtf8Readonly",
                content: "legacy",
                encoding: "windows-1252",
                size: 6
            }
        })
        documentGeneration.mockReturnValue(1)
        render(<EditorPane path={PATH} groupIndex={0} />)
        await waitFor(() => expect(registerView).toHaveBeenCalled())
        const view = registerView.mock.calls[0][1] as EditorView

        view.contentDOM.dispatchEvent(
            new KeyboardEvent("keydown", {
                key: "s",
                code: "KeyS",
                ctrlKey: true,
                bubbles: true,
                cancelable: true
            })
        )
        await act(async () => Promise.resolve())

        expect(saveFile).not.toHaveBeenCalled()
    })

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

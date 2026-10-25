import { act, cleanup, render, screen } from "@testing-library/react"
import { useEffect } from "react"
import { afterEach, expect, it, vi } from "vitest"

import { clearAll, reloadDocument } from "@/editor/documentRegistry"
import { useWorkspaceStore } from "@/state/workspaceStore"

vi.mock("@/lib/ipc", async (importOriginal) => ({
    ...(await importOriginal<typeof import("@/lib/ipc")>()),
    openFileSnapshot: vi.fn(async () => ({ result: { kind: "binary", size: 2048 }, accept: () => {} }))
}))

vi.mock("./TabBar", () => ({ TabBar: () => <div data-testid="tab-bar" /> }))
vi.mock("../editor/EditorPane", () => ({ EditorPane: () => <div data-testid="editor-pane" /> }))
vi.mock("@/app/panels/HerdrTerminalPage", () => ({ HerdrTerminalPage: () => null }))
vi.mock("@/app/panels/PreviewPanel", () => ({ PreviewPanel: () => null }))

const mounts = vi.hoisted(() => ({ count: 0 }))
vi.mock("./PdfView", async (importOriginal) => ({
    ...(await importOriginal<typeof import("./PdfView")>()),
    PdfView: ({ path, generation }: { path: string; generation?: number }) => {
        useEffect(() => {
            mounts.count++
        }, [])
        return <div data-testid="pdf-view" data-path={path} data-generation={generation} />
    }
}))

import { EditorArea } from "./EditorArea"

const initialWorkspaceState = useWorkspaceStore.getState()

afterEach(() => {
    cleanup()
    clearAll()
    useWorkspaceStore.setState(initialWorkspaceState, true)
    mounts.count = 0
})

it("opens .pdf tabs in the PDF viewer and reloads them in place after an external rewrite", async () => {
    const path = "/w/docs/manual.PDF"
    useWorkspaceStore.setState({
        workspacePath: "/w",
        groups: [{ activePath: path, tabs: [{ path, name: "manual.PDF", dirty: false, externallyModified: false }] }],
        activeGroupIndex: 0
    })
    render(<EditorArea />)
    const view = screen.getByTestId("pdf-view")
    expect(view).toHaveAttribute("data-path", path)
    expect(screen.queryByTestId("editor-pane")).toBeNull()
    const before = Number(view.dataset.generation)

    // What ExternalChangeBridge does for a clean tab.
    await act(async () => {
        await reloadDocument(path, () => true, "reconcile")
        useWorkspaceStore.getState().markExternallyModified(path, false)
    })

    expect(Number(screen.getByTestId("pdf-view").dataset.generation)).toBeGreaterThan(before)
    expect(mounts.count).toBe(1)
})

import { afterEach, beforeEach, expect, it, vi } from "vitest"
import { useState } from "react"
import { act, cleanup, fireEvent, render, screen } from "@testing-library/react"

vi.mock("@/lsp/lspManager", () => ({ ensureClient: vi.fn() }))
vi.mock("@/lsp/symbols", () => ({
    requestDocumentSymbols: vi.fn(),
    requestWorkspaceSymbols: vi.fn()
}))
vi.mock("@/editor/documentRegistry", () => ({ getDocument: vi.fn() }))

import { CommandPalette } from "@/app/workbench/CommandPalette"
import { ensureClient } from "@/lsp/lspManager"
import { requestDocumentSymbols, requestWorkspaceSymbols } from "@/lsp/symbols"
import { getDocument } from "@/editor/documentRegistry"
import { uiInitialState, useUiStore } from "@/state/uiStore"
import { useWorkspaceStore } from "@/state/workspaceStore"

const managed = {
    client: { id: "fake", initializing: Promise.resolve() },
    language: "typescript",
    capabilities: { documentSymbolProvider: true, workspaceSymbolProvider: true }
}

beforeEach(() => {
    useUiStore.setState(uiInitialState)
    vi.mocked(ensureClient).mockResolvedValue(managed as never)
    vi.mocked(requestDocumentSymbols).mockResolvedValue([])
    vi.mocked(requestWorkspaceSymbols).mockResolvedValue([])
    vi.mocked(getDocument).mockResolvedValue({ result: { kind: "full", content: "", size: 0 } })
    useWorkspaceStore.setState({
        workspacePath: "/ws",
        groups: [{ tabs: [], activePath: "/ws/a.ts" }],
        activeGroupIndex: 0,
        pendingReveal: null
    })
})

afterEach(() => {
    cleanup()
    vi.clearAllMocks()
})

const flush = async () => {
    await act(async () => {
        for (let i = 0; i < 6; i++) await Promise.resolve()
    })
}

// Owns the palette open state the way the workbench does, so the "Go to symbol"
// entry (which closes the palette) and the ⌘K handler operate on real state.
function Harness() {
    const [open, setOpen] = useState(true)
    return (
        <CommandPalette
            open={open}
            onOpenChange={setOpen}
            onSelectMode={() => {}}
            onOpenSettings={() => {}}
        />
    )
}

it("⌘K while the symbol picker is open closes the picker without stacking the palette", async () => {
    render(<Harness />)

    // Open the symbol picker from the palette: the palette closes, the picker opens.
    fireEvent.click(await screen.findByRole("option", { name: /go to symbol/i }))
    await flush()
    expect(screen.getAllByRole("dialog")).toHaveLength(1)

    // ⌘K must close the picker and NOT open the palette on top of it.
    await act(async () => {
        fireEvent.keyDown(window, { key: "k", metaKey: true })
    })
    await flush()
    expect(screen.queryAllByRole("dialog")).toHaveLength(0)
})

it("renders terminal and preview toggles and closes after selection", async () => {
    render(<Harness />)

    expect(await screen.findByRole("option", { name: /toggle terminal/i })).toBeInTheDocument()
    expect(screen.getByRole("option", { name: /toggle preview/i })).toBeInTheDocument()

    fireEvent.click(screen.getByRole("option", { name: /toggle terminal/i }))
    expect(useUiStore.getState().terminalOpen).toBe(true)
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument()
})

it("toggle preview command flips preview visibility", async () => {
    render(<Harness />)

    fireEvent.click(await screen.findByRole("option", { name: /toggle preview/i }))

    expect(useUiStore.getState().previewOpen).toBe(true)
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument()
})

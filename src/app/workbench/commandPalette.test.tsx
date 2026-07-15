import { afterEach, beforeEach, expect, it, vi } from "vitest"
import { useState } from "react"
import { act, cleanup, fireEvent, render, screen } from "@testing-library/react"

vi.mock("@/lsp/lspManager", () => ({ ensureClient: vi.fn() }))
vi.mock("@/lsp/symbols", () => ({
    requestDocumentSymbols: vi.fn(),
    requestWorkspaceSymbols: vi.fn()
}))
vi.mock("@/editor/documentRegistry", () => ({ getDocument: vi.fn() }))

const searchWorkspace = vi.fn(
    (_root: string, _query: string, _cs: boolean, _cb: (e: SearchEvent) => void) => Promise.resolve()
)
vi.mock("@/lib/ipc", async (importOriginal) => ({
    ...(await importOriginal<typeof import("@/lib/ipc")>()),
    searchWorkspace: (...args: Parameters<typeof searchWorkspace>) => searchWorkspace(...args),
}))

import type { SearchEvent } from "@/lib/types"
import { CommandPalette } from "@/app/workbench/CommandPalette"
import { ensureClient } from "@/lsp/lspManager"
import { requestDocumentSymbols, requestWorkspaceSymbols } from "@/lsp/symbols"
import { getDocument } from "@/editor/documentRegistry"
import { uiInitialState, useUiStore } from "@/state/uiStore"
import { PREVIEW_TAB_PATH, useWorkspaceStore } from "@/state/workspaceStore"

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
    vi.mocked(getDocument).mockResolvedValue({ result: { kind: "full", content: "", size: 0, lineEnding: "lf" } })
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

it("toggle preview command opens the singleton preview tab", async () => {
    render(<Harness />)

    fireEvent.click(await screen.findByRole("option", { name: /toggle preview/i }))

    const groups = useWorkspaceStore.getState().groups
    expect(groups.some((g) => g.tabs.some((t) => t.path === PREVIEW_TAB_PATH))).toBe(true)
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument()
})

it("'>' prefix restricts to commands and skips the workspace search", async () => {
    render(<Harness />)
    const input = await screen.findByPlaceholderText("Search files, run a command…")

    // ">" alone keeps every command and runs no workspace search.
    fireEvent.change(input, { target: { value: ">" } })
    expect(screen.getByRole("option", { name: /toggle terminal/i })).toBeInTheDocument()
    expect(screen.queryByText("Workspace search")).not.toBeInTheDocument()
    expect(searchWorkspace).not.toHaveBeenCalled()

    // The text after ">" filters the command list.
    fireEvent.change(input, { target: { value: ">settings" } })
    expect(screen.getByRole("option", { name: /settings/i })).toBeInTheDocument()
    expect(screen.queryByRole("option", { name: /toggle terminal/i })).not.toBeInTheDocument()
})

it("a single-character query stays below the search floor and runs no workspace search", async () => {
    render(<Harness />)
    const input = await screen.findByPlaceholderText("Search files, run a command…")

    fireEvent.change(input, { target: { value: "a" } })
    expect(searchWorkspace).not.toHaveBeenCalled()
    expect(screen.queryByText("Workspace search")).not.toBeInTheDocument()
})

it("a plain query renders the workspace search group and reveals a hit on select", async () => {
    vi.useFakeTimers()
    searchWorkspace.mockImplementation((_r, _q, _cs, cb) => {
        cb({ type: "match", path: "/ws/src/a.ts", matches: [{ line: 3, col: 2, preview: "a needle b" }] })
        cb({ type: "done", truncated: false, fileCount: 1 })
        return Promise.resolve()
    })

    render(<Harness />)
    const input = screen.getByPlaceholderText("Search files, run a command…")
    fireEvent.change(input, { target: { value: "needle" } })
    await act(async () => {
        await vi.advanceTimersByTimeAsync(250)
    })

    expect(searchWorkspace).toHaveBeenCalledWith("/ws", "needle", false, expect.any(Function))
    expect(screen.getByText("Workspace search")).toBeInTheDocument()
    expect(screen.getByText("a.ts")).toBeInTheDocument()
    expect(screen.getByText("needle").tagName).toBe("MARK")

    fireEvent.click(screen.getByText("needle"))
    expect(useWorkspaceStore.getState().pendingReveal).toEqual({ path: "/ws/src/a.ts", line: 3 })
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument()
    vi.useRealTimers()
})

it("workspace search sanitizes an extended Windows child path but reveals the raw target", async () => {
    vi.useFakeTimers()
    const workspace = String.raw`\\?\C:\Work\中文 workspace`
    const rawPath = String.raw`\\?\C:\Work\中文 workspace\src\a.ts`
    useWorkspaceStore.setState({ workspacePath: workspace })
    searchWorkspace.mockImplementation((_r, _q, _cs, cb) => {
        cb({ type: "match", path: rawPath, matches: [{ line: 3, col: 2, preview: "a needle b" }] })
        cb({ type: "done", truncated: false, fileCount: 1 })
        return Promise.resolve()
    })

    render(<Harness />)
    fireEvent.change(screen.getByPlaceholderText("Search files, run a command…"), {
        target: { value: "needle" }
    })
    await act(async () => {
        await vi.advanceTimersByTimeAsync(250)
    })

    expect(searchWorkspace).toHaveBeenCalledWith(workspace, "needle", false, expect.any(Function))
    expect(screen.getByText("a.ts")).toBeInTheDocument()
    expect(screen.getByText(String.raw`src\a.ts`)).toBeInTheDocument()
    expect(screen.queryByText(rawPath)).not.toBeInTheDocument()

    fireEvent.click(screen.getByText("needle"))
    expect(useWorkspaceStore.getState().pendingReveal).toEqual({ path: rawPath, line: 3 })
    vi.useRealTimers()
})

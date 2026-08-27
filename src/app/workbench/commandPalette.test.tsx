import { afterEach, beforeEach, expect, it, vi } from "vitest"
import { useState } from "react"
import { act, cleanup, fireEvent, render, screen } from "@testing-library/react"

vi.mock("@/lsp/lspManager", () => ({ ensureClient: vi.fn() }))
vi.mock("@/lsp/symbols", () => ({
    requestDocumentSymbols: vi.fn(),
    requestWorkspaceSymbols: vi.fn()
}))
vi.mock("@/editor/documentRegistry", () => ({ getDocument: vi.fn() }))
vi.mock("@/lib/actionFeedback", () => ({ showActionError: vi.fn() }))

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
import { showActionError } from "@/lib/actionFeedback"
import { herdrInitialState, useHerdrStore } from "@/state/herdrStore"
import { uiInitialState, useUiStore } from "@/state/uiStore"
import { markdownPreviewPath } from "@/lib/markdownPreviewTab"
import { PREVIEW_TAB_PATH, useWorkspaceStore } from "@/state/workspaceStore"

const managed = {
    client: { id: "fake", initializing: Promise.resolve() },
    language: "typescript",
    capabilities: { documentSymbolProvider: true, workspaceSymbolProvider: true }
}

beforeEach(() => {
    useUiStore.setState(uiInitialState)
    useHerdrStore.setState({ ...herdrInitialState, attachments: new Map() })
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
    useHerdrStore.setState({ ...herdrInitialState, attachments: new Map() })
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

it("lists existing Herdr tabs as commands and activates the selected tab", async () => {
    const activateTab = vi.fn().mockResolvedValue({ ok: true })
    useHerdrStore.setState({
        selectedSessionName: "default",
        selectedSpaceId: "ws-1",
        snapshot: {
            herdrSessionId: "default",
            protocol: 19,
            version: "0.8.0",
            spaces: [{ id: "ws-1", label: "Main", order: 1, focused: true }],
            agents: [],
            tabs: [
                {
                    id: "tab-1",
                    label: "Agent",
                    order: 1,
                    workspaceId: "ws-1",
                    paneCount: 2,
                    status: "idle",
                    active: true,
                    focused: true,
                    paneId: "pane-1",
                    terminalId: "term-1",
                    sessionName: "default"
                }
            ],
            terminals: [],
            focusedWorkspaceId: "ws-1",
            focusedTabId: "tab-1",
            focusedPaneId: "pane-1",
            raw: {}
        },
        canMutateSelectedSession: () => true,
        canFocusSelectedTab: () => true,
        activateTab
    })
    render(<Harness />)

    fireEvent.click(await screen.findByRole("option", { name: "Open Herdr tab: Agent" }))

    expect(activateTab).toHaveBeenCalledWith(
        expect.objectContaining({ id: "tab-1", terminalId: "term-1" })
    )
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument()
})

it("lists Herdr Agents by urgency and activates them from global search", async () => {
    const activateAgent = vi.fn().mockResolvedValue({ ok: true })
    useHerdrStore.setState({
        selectedSessionName: "default",
        selectedSpaceId: "ws-1",
        snapshot: {
            herdrSessionId: "default",
            protocol: 19,
            version: "0.8.0",
            spaces: [{ id: "ws-1", label: "Main", order: 1, focused: true }],
            agents: [
                { id: "idle", name: "Idle", status: "idle", workspaceId: "ws-1" },
                { id: "blocked", name: "Blocked", status: "blocked", workspaceId: "ws-1" }
            ],
            tabs: [],
            terminals: [],
            raw: {}
        },
        canMutateSelectedSession: () => true,
        canFocusSelectedTab: () => false,
        activateAgent
    })
    render(<Harness />)

    const blocked = await screen.findByRole("option", {
        name: "Open Herdr Agent: Blocked · blocked"
    })
    const idle = screen.getByRole("option", {
        name: "Open Herdr Agent: Idle · idle"
    })
    expect(blocked.compareDocumentPosition(idle) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy()

    fireEvent.click(blocked)
    expect(activateAgent).toHaveBeenCalledWith(
        expect.objectContaining({ id: "blocked", status: "blocked" })
    )
})

it("hides tab-bound Agents when tab.focus is unavailable but keeps Space-level Agents", async () => {
    useHerdrStore.setState({
        selectedSessionName: "default",
        selectedSpaceId: "ws-1",
        snapshot: {
            herdrSessionId: "default",
            protocol: 19,
            version: "0.8.0",
            spaces: [{ id: "ws-1", label: "Main", order: 1, focused: true }],
            agents: [
                {
                    id: "tab-bound",
                    name: "Tab bound",
                    status: "working",
                    workspaceId: "ws-1",
                    tabId: "tab-1",
                    terminalId: "term-1"
                },
                {
                    id: "space-level",
                    name: "Space level",
                    status: "idle",
                    workspaceId: "ws-1",
                    terminalId: "term-2"
                }
            ],
            tabs: [],
            terminals: [],
            raw: {}
        },
        canMutateSelectedSession: () => true,
        canFocusSelectedTab: () => false
    })
    render(<Harness />)

    expect(screen.queryByRole("option", {
        name: "Open Herdr Agent: Tab bound · working"
    })).not.toBeInTheDocument()
    expect(await screen.findByRole("option", {
        name: "Open Herdr Agent: Space level · idle"
    })).toBeInTheDocument()
})

it("surfaces non-cancelled Herdr Agent activation failures", async () => {
    const activateAgent = vi.fn().mockResolvedValue({
        ok: false,
        error: "herdr tab.focus unavailable"
    })
    useHerdrStore.setState({
        selectedSessionName: "default",
        selectedSpaceId: "ws-1",
        snapshot: {
            herdrSessionId: "default",
            protocol: 19,
            version: "0.8.0",
            spaces: [{ id: "ws-1", label: "Main", order: 1, focused: true }],
            agents: [{
                id: "tab-bound",
                name: "Tab bound",
                status: "working",
                workspaceId: "ws-1",
                tabId: "tab-1",
                terminalId: "term-1"
            }],
            tabs: [],
            terminals: [],
            raw: {}
        },
        canMutateSelectedSession: () => true,
        canFocusSelectedTab: () => true,
        activateAgent
    })
    render(<Harness />)

    fireEvent.click(await screen.findByRole("option", {
        name: "Open Herdr Agent: Tab bound · working"
    }))

    await vi.waitFor(() => {
        expect(showActionError).toHaveBeenCalledWith(
            "Open Herdr Agent: Tab bound · working",
            "herdr tab.focus unavailable"
        )
    })
})

it("lists Herdr Spaces and activates the selected runtime namespace", async () => {
    const activateSpace = vi.fn().mockResolvedValue({ ok: true })
    useHerdrStore.setState({
        selectedSessionName: "work",
        selectedSpaceId: "ws-1",
        snapshot: {
            herdrSessionId: "work",
            protocol: 19,
            version: "0.8.0",
            spaces: [
                {
                    id: "ws-2",
                    label: "Feature",
                    order: 2,
                    focused: false,
                    path: String.raw`C:\Work\Feature`
                }
            ],
            agents: [],
            tabs: [],
            terminals: [],
            raw: {}
        },
        canMutateSelectedSession: () => true,
        canFocusSelectedTab: () => false,
        activateSpace
    })
    render(<Harness />)

    fireEvent.click(
        await screen.findByRole("option", { name: "Open Herdr Space: Feature" })
    )
    expect(activateSpace).toHaveBeenCalledWith({
        sessionName: "work",
        workspaceId: "ws-2",
        path: String.raw`C:\Work\Feature`
    })
})

it("filters the complete Herdr Space snapshot by name or ID before applying the display cap", async () => {
    useHerdrStore.setState({
        selectedSessionName: "work",
        selectedSpaceId: "ws-0",
        snapshot: {
            herdrSessionId: "work",
            protocol: 19,
            version: "0.8.0",
            spaces: [
                ...Array.from({ length: 64 }, (_, index) => ({
                    id: `ws-${index}`,
                    label: `Space ${index}`,
                    order: index,
                    focused: index === 0
                })),
                { id: "ws-late", label: "Late Space", order: 64, focused: false }
            ],
            agents: [],
            tabs: [],
            terminals: [],
            raw: {}
        },
        canMutateSelectedSession: () => true,
        canFocusSelectedTab: () => false
    })
    render(<Harness />)

    const input = await screen.findByPlaceholderText("Search files, run a command…")
    fireEvent.change(input, {
        target: { value: ">Late Space" }
    })

    expect(screen.getByRole("option", { name: "Open Herdr Space: Late Space" })).toBeInTheDocument()

    fireEvent.change(input, { target: { value: ">ws-late" } })

    expect(screen.getByRole("option", { name: "Open Herdr Space: Late Space" })).toBeInTheDocument()
})

it("filters the complete Herdr Agent snapshot by name or ID before applying the display cap", async () => {
    useHerdrStore.setState({
        selectedSessionName: "default",
        selectedSpaceId: "ws-1",
        snapshot: {
            herdrSessionId: "default",
            protocol: 19,
            version: "0.8.0",
            spaces: [{ id: "ws-1", label: "Main", order: 0, focused: true }],
            agents: [
                ...Array.from({ length: 128 }, (_, index) => ({
                    id: `blocked-${index}`,
                    name: `Blocked ${index}`,
                    status: "blocked" as const,
                    workspaceId: "ws-1"
                })),
                {
                    id: "agent-late",
                    name: "Late Agent",
                    status: "idle" as const,
                    workspaceId: "ws-1"
                }
            ],
            tabs: [],
            terminals: [],
            raw: {}
        },
        canMutateSelectedSession: () => true,
        canFocusSelectedTab: () => false
    })
    render(<Harness />)

    const input = await screen.findByPlaceholderText("Search files, run a command…")
    fireEvent.change(input, {
        target: { value: ">Late Agent" }
    })

    expect(screen.getByRole("option", {
        name: "Open Herdr Agent: Late Agent · idle"
    })).toBeInTheDocument()

    fireEvent.change(input, { target: { value: ">agent-late" } })

    expect(screen.getByRole("option", {
        name: "Open Herdr Agent: Late Agent · idle"
    })).toBeInTheDocument()
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

it("Go to symbol with an active markdown preview does not start document/LSP lookup", async () => {
    const previewPath = markdownPreviewPath("/ws/readme.md")
    useWorkspaceStore.setState({
        workspacePath: "/ws",
        groups: [{
            activePath: previewPath,
            tabs: [{
                path: previewPath,
                name: "Preview",
                dirty: false,
                externallyModified: false,
                kind: "markdown-preview",
                sourcePath: "/ws/readme.md"
            }]
        }],
        activeGroupIndex: 0
    })

    render(<Harness />)
    fireEvent.click(await screen.findByRole("option", { name: /go to symbol/i }))
    await flush()

    expect(getDocument).not.toHaveBeenCalled()
    expect(ensureClient).not.toHaveBeenCalled()
    expect(requestDocumentSymbols).not.toHaveBeenCalled()
})

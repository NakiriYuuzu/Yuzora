import { afterEach, beforeEach, expect, it, vi } from "vitest"
import { act, cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react"

vi.mock("../lsp/lspManager", () => ({ ensureClient: vi.fn() }))
vi.mock("../lsp/symbols", () => ({
    requestDocumentSymbols: vi.fn(),
    requestWorkspaceSymbols: vi.fn()
}))
vi.mock("../editor/documentRegistry", () => ({ getDocument: vi.fn() }))

import { SymbolPicker } from "./SymbolPicker"
import { ensureClient } from "../lsp/lspManager"
import { requestDocumentSymbols, requestWorkspaceSymbols } from "../lsp/symbols"
import { getDocument } from "../editor/documentRegistry"
import i18n from "../lib/i18n"
import { useWorkspaceStore } from "../state/workspaceStore"

const managed = {
    client: { id: "fake", initializing: Promise.resolve() },
    language: "typescript",
    capabilities: { documentSymbolProvider: true, workspaceSymbolProvider: true }
}

beforeEach(() => {
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
    vi.useRealTimers()
})

const flush = async (ms = 0) => {
    await act(async () => {
        if (ms) vi.advanceTimersByTime(ms)
        for (let i = 0; i < 6; i++) await Promise.resolve()
    })
}

it("waits for the initialize handshake before deciding there is no provider (cold start)", async () => {
    // ensureClient resolves as soon as the transport connects, with capabilities
    // still null — lspManager fills them in a fire-and-forget .then on
    // client.initializing (rust-analyzer can take seconds). The picker must await
    // that handshake, else a cold server is misread as "no provider" → empty, and
    // never recovers because [open, mode] don't re-run when capabilities arrive.
    let resolveInit: () => void = () => {}
    const initializing = new Promise<void>((r) => {
        resolveInit = r
    })
    const coldManaged = {
        client: { id: "cold", initializing },
        language: "typescript",
        capabilities: null as { documentSymbolProvider?: unknown } | null
    }
    // Mirror lspManager: capabilities appear only after initializing resolves, via
    // a .then registered before the picker awaits the same promise.
    void initializing.then(() => {
        coldManaged.capabilities = { documentSymbolProvider: true }
    })
    vi.mocked(ensureClient).mockResolvedValue(coldManaged as never)
    vi.mocked(requestDocumentSymbols).mockResolvedValue([
        { name: "coldFn", kind: 12, range: { start: { line: 2, character: 0 }, end: { line: 2, character: 3 } } }
    ])

    render(<SymbolPicker open onOpenChange={() => {}} mode="document" />)

    // Handshake still pending → no documentSymbol request may go out yet.
    await flush()
    expect(requestDocumentSymbols).not.toHaveBeenCalled()

    // Resolve the handshake → capabilities populate → the request fires and the
    // outline renders, no reopen required.
    await act(async () => {
        resolveInit()
        for (let i = 0; i < 6; i++) await Promise.resolve()
    })
    await waitFor(() =>
        expect(requestDocumentSymbols).toHaveBeenCalledWith(coldManaged.client, "file:///ws/a.ts")
    )
    expect(await screen.findByRole("option", { name: /coldFn/i })).toBeInTheDocument()
})

it("document mode lists the outline and reveals the 1-based line on select", async () => {
    vi.mocked(requestDocumentSymbols).mockResolvedValue([
        { name: "myFunc", kind: 12, range: { start: { line: 4, character: 0 }, end: { line: 4, character: 6 } } }
    ])
    const revealSpy = vi
        .spyOn(useWorkspaceStore.getState(), "requestReveal")
        .mockImplementation(() => {})

    render(<SymbolPicker open onOpenChange={() => {}} mode="document" />)

    await waitFor(() =>
        expect(requestDocumentSymbols).toHaveBeenCalledWith(managed.client, "file:///ws/a.ts")
    )
    fireEvent.click(await screen.findByRole("option", { name: /myFunc/i }))

    expect(revealSpy).toHaveBeenCalledWith("/ws/a.ts", 5)
})

it("workspace mode debounces the query, requests workspace symbols, and opens on select", async () => {
    vi.useFakeTimers()
    vi.mocked(requestWorkspaceSymbols).mockResolvedValue([
        {
            name: "Widget",
            kind: 5,
            uri: "file:///ws/b.ts",
            range: { start: { line: 9, character: 0 }, end: { line: 9, character: 6 } }
        }
    ])
    // requestReveal opens the tab internally, so the picker relies on it rather
    // than calling openTab directly (B3) — assert the reveal, not a second openTab.
    const revealSpy = vi
        .spyOn(useWorkspaceStore.getState(), "requestReveal")
        .mockImplementation(() => {})

    render(<SymbolPicker open onOpenChange={() => {}} mode="workspace" />)

    const input = screen.getByPlaceholderText(i18n.t("workspaceSymbolsPlaceholder", { ns: "lsp" }))
    await act(async () => {
        fireEvent.change(input, { target: { value: "Wid" } })
    })
    // no request until the debounce window elapses
    expect(requestWorkspaceSymbols).not.toHaveBeenCalled()

    await flush(250)
    expect(requestWorkspaceSymbols).toHaveBeenCalledWith(managed.client, "Wid")

    fireEvent.click(screen.getByRole("option", { name: /Widget/i }))
    expect(revealSpy).toHaveBeenCalledWith("/ws/b.ts", 10)
})

it("document mode with no documentSymbolProvider capability stays empty and sends no request", async () => {
    vi.mocked(ensureClient).mockResolvedValue({
        client: { id: "fake", initializing: Promise.resolve() },
        language: "typescript",
        capabilities: { workspaceSymbolProvider: true }
    } as never)

    render(<SymbolPicker open onOpenChange={() => {}} mode="document" />)
    await flush()

    expect(requestDocumentSymbols).not.toHaveBeenCalled()
    expect(screen.queryByRole("option")).not.toBeInTheDocument()
})

it("workspace mode with no workspaceSymbolProvider capability stays empty and sends no request", async () => {
    vi.useFakeTimers()
    vi.mocked(ensureClient).mockResolvedValue({
        client: { id: "fake", initializing: Promise.resolve() },
        language: "typescript",
        capabilities: { documentSymbolProvider: true }
    } as never)

    render(<SymbolPicker open onOpenChange={() => {}} mode="workspace" />)
    const input = screen.getByPlaceholderText(i18n.t("workspaceSymbolsPlaceholder", { ns: "lsp" }))
    await act(async () => {
        fireEvent.change(input, { target: { value: "Wid" } })
    })
    await flush(250)

    expect(requestWorkspaceSymbols).not.toHaveBeenCalled()
    expect(screen.queryByRole("option")).not.toBeInTheDocument()
})

it("clears the query when reopened without remounting (reset-on-close effect is reachable)", async () => {
    const { rerender } = render(<SymbolPicker open onOpenChange={() => {}} mode="document" />)
    await flush()

    const input = () => screen.getByPlaceholderText(i18n.t("goToSymbolPlaceholder", { ns: "lsp" }))
    await act(async () => {
        fireEvent.change(input(), { target: { value: "xyz" } })
    })
    expect(input()).toHaveValue("xyz")

    // Close (open=false) then reopen — the component stays mounted, so the
    // reset-on-close effect (not a remount) is what clears the query.
    rerender(<SymbolPicker open={false} onOpenChange={() => {}} mode="document" />)
    await flush()
    rerender(<SymbolPicker open onOpenChange={() => {}} mode="document" />)
    await flush()

    expect(input()).toHaveValue("")
})

it("gates out a non-full file (tooLarge): no client spin-up, no request, empty list", async () => {
    // resolveActive must apply the same file-grade gate as EditorPane — a tooLarge
    // (or binary / limited / nonUtf8 / very-long-line) file must not spin up a
    // server the grade rejects nor query a URI that was never didOpen'd.
    vi.mocked(getDocument).mockResolvedValue({ result: { kind: "tooLarge", size: 20_000_000 } })

    render(<SymbolPicker open onOpenChange={() => {}} mode="document" />)
    await flush()

    expect(ensureClient).not.toHaveBeenCalled()
    expect(requestDocumentSymbols).not.toHaveBeenCalled()
    expect(screen.queryByRole("option")).not.toBeInTheDocument()
})

it("workspace mode waits for the initialize handshake before deciding there is no provider (cold start)", async () => {
    // Mirror of the document cold-start guarantee for workspace mode: capabilities
    // arrive only after client.initializing resolves, so the workspace effect must
    // await it before reading workspaceSymbolProvider — else a cold server is
    // misread as "no provider" → empty, and never recovers ([open, mode, query]
    // don't re-run when capabilities arrive).
    vi.useFakeTimers()
    let resolveInit: () => void = () => {}
    const initializing = new Promise<void>((r) => {
        resolveInit = r
    })
    const coldManaged = {
        client: { id: "cold", initializing },
        language: "typescript",
        capabilities: null as { workspaceSymbolProvider?: unknown } | null
    }
    void initializing.then(() => {
        coldManaged.capabilities = { workspaceSymbolProvider: true }
    })
    vi.mocked(ensureClient).mockResolvedValue(coldManaged as never)
    vi.mocked(requestWorkspaceSymbols).mockResolvedValue([
        {
            name: "coldSym",
            kind: 5,
            uri: "file:///ws/b.ts",
            range: { start: { line: 3, character: 0 }, end: { line: 3, character: 3 } }
        }
    ])

    render(<SymbolPicker open onOpenChange={() => {}} mode="workspace" />)
    const input = screen.getByPlaceholderText(i18n.t("workspaceSymbolsPlaceholder", { ns: "lsp" }))
    await act(async () => {
        fireEvent.change(input, { target: { value: "cold" } })
    })
    // Debounce elapses and the effect runs, but the handshake is still pending →
    // no workspace/symbol request may go out yet.
    await flush(250)
    expect(requestWorkspaceSymbols).not.toHaveBeenCalled()

    // Resolve the handshake → capabilities populate → the request fires and the
    // results render, no reopen required.
    await act(async () => {
        resolveInit()
        for (let i = 0; i < 6; i++) await Promise.resolve()
    })
    await flush()
    expect(requestWorkspaceSymbols).toHaveBeenCalledWith(coldManaged.client, "cold")
    expect(screen.getByRole("option", { name: /coldSym/i })).toBeInTheDocument()
})

it("degrades to the empty state when getDocument rejects (no unhandled rejection)", async () => {
    // The active file may be deleted / become unreadable between opening the
    // picker and resolveActive's getDocument. That reject must fall back to the
    // empty state, not escape the effect's async IIFE as an unhandled rejection.
    vi.mocked(getDocument).mockRejectedValue(new Error("file gone"))

    render(<SymbolPicker open onOpenChange={() => {}} mode="document" />)
    await flush()

    expect(ensureClient).not.toHaveBeenCalled()
    expect(requestDocumentSymbols).not.toHaveBeenCalled()
    expect(screen.queryByRole("option")).not.toBeInTheDocument()
})

it("document mode degrades to the empty state when requestDocumentSymbols rejects (no unhandled rejection)", async () => {
    // A per-request failure (server error like ContentModified -32801, a client
    // timeout, or "Client not connected" on workspace switch) isn't caught by the
    // capability gate — it must degrade to the empty list, not escape the effect's
    // async IIFE as an unhandled rejection.
    vi.mocked(requestDocumentSymbols).mockRejectedValue({ code: -32801, message: "content modified" })

    render(<SymbolPicker open onOpenChange={() => {}} mode="document" />)
    await flush()

    expect(requestDocumentSymbols).toHaveBeenCalled()
    expect(screen.queryByRole("option")).not.toBeInTheDocument()
})

it("workspace mode degrades to the empty state when requestWorkspaceSymbols rejects (no unhandled rejection)", async () => {
    vi.useFakeTimers()
    vi.mocked(requestWorkspaceSymbols).mockRejectedValue({ code: -32801, message: "content modified" })

    render(<SymbolPicker open onOpenChange={() => {}} mode="workspace" />)
    const input = screen.getByPlaceholderText(i18n.t("workspaceSymbolsPlaceholder", { ns: "lsp" }))
    await act(async () => {
        fireEvent.change(input, { target: { value: "Wid" } })
    })
    await flush(250)

    expect(requestWorkspaceSymbols).toHaveBeenCalled()
    expect(screen.queryByRole("option")).not.toBeInTheDocument()
})

it("workspace mode is not disabled by a non-full active file (grade gate is document-only)", async () => {
    // workspace/symbol is a cross-file query that never touches the active URI, so
    // an oversized/limited active file must not disable it (unlike document mode).
    vi.useFakeTimers()
    vi.mocked(getDocument).mockResolvedValue({ result: { kind: "tooLarge", size: 20_000_000 } })
    vi.mocked(requestWorkspaceSymbols).mockResolvedValue([
        {
            name: "Widget",
            kind: 5,
            uri: "file:///ws/b.ts",
            range: { start: { line: 9, character: 0 }, end: { line: 9, character: 6 } }
        }
    ])

    render(<SymbolPicker open onOpenChange={() => {}} mode="workspace" />)
    const input = screen.getByPlaceholderText(i18n.t("workspaceSymbolsPlaceholder", { ns: "lsp" }))
    await act(async () => {
        fireEvent.change(input, { target: { value: "Wid" } })
    })
    await flush(250)

    expect(requestWorkspaceSymbols).toHaveBeenCalledWith(managed.client, "Wid")
    expect(screen.getByRole("option", { name: /Widget/i })).toBeInTheDocument()
})

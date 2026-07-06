import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"
import { act, cleanup, render, waitFor } from "@testing-library/react"
import type { EditorView } from "@codemirror/view"

import { useWorkspaceStore } from "../state/workspaceStore"

// Spy on the view registry so we can grab the live view and assert timing.
const registerView = vi.fn()
const unregisterView = vi.fn()
vi.mock("./viewRegistry", () => ({
    registerView: (path: string, view: EditorView) => registerView(path, view),
    unregisterView: (path: string, view?: EditorView) => unregisterView(path, view),
    getView: vi.fn()
}))

const getDocument = vi.fn(async (_path: string) => ({
    result: { kind: "full", content: "const x = 1\n", size: 12 }
}))
vi.mock("./documentRegistry", () => ({
    getDocument: (path: string) => getDocument(path),
    updateBuffer: vi.fn(),
    documentGeneration: vi.fn(() => 0)
}))

const saveFile = vi.fn(async (_path: string, _content: string) => 0)
vi.mock("../lib/ipc", () => ({
    saveFile: (path: string, content: string) => saveFile(path, content)
}))

vi.mock("@/features/logs/userAction", () => ({
    logUserAction: vi.fn(async () => undefined)
}))

vi.mock("../workbench/ExternalChangeResolver", () => ({
    maybeInterceptSave: () => false
}))

// Mock the manager: lspExtensionsForFile is the single mount-gating seam (grade
// gating + capability-ready assembly live inside it, covered in lspManager.test).
// It hands back the fake ManagedClient + assembled extensions for a full-grade
// file, null otherwise. The two pure predicates keep their real behaviour so the
// save path is exercised for real.
const lspExtensionsForFile = vi.fn()
const clientSync = vi.fn()
const clientRequest = vi.fn(async () => [] as unknown)
const managed = {
    client: { sync: clientSync, request: clientRequest },
    language: "typescript" as const,
    capabilities: { documentFormattingProvider: true }
}
vi.mock("../lsp/lspManager", () => ({
    lspExtensionsForFile: (path: string, grade: string) => lspExtensionsForFile(path, grade),
    flushPendingChanges: (m: typeof managed) => m.client.sync(),
    shouldFormatOnSave: (caps: { documentFormattingProvider?: unknown } | null, enabled: boolean) =>
        enabled && !!caps?.documentFormattingProvider
}))

const { EditorPane, FORMAT_ON_SAVE_STORAGE_KEY } = await import("./EditorPane")

const PATH = "/w/a.ts"

function makeDeferred<T>() {
    let resolve!: (v: T) => void
    const promise = new Promise<T>((r) => {
        resolve = r
    })
    return { promise, resolve }
}

// The async LSP mount awaits lspExtensionsForFile; let it resolve so managedRef
// (consumed by save for flush + format gating) is set before we trigger a save.
async function flushMount() {
    await act(async () => {
        await new Promise((r) => setTimeout(r, 0))
    })
}

// The Bun-hosted test runtime injects an empty `localStorage` global with no
// Storage methods (see gitStore.test.ts); install a minimal in-memory Storage so
// the format-on-save setting is read/written for real.
function installLocalStorage(): void {
    const store = new Map<string, string>()
    Object.defineProperty(globalThis, "localStorage", {
        value: {
            getItem: (k: string) => (store.has(k) ? store.get(k)! : null),
            setItem: (k: string, v: string) => void store.set(k, String(v)),
            removeItem: (k: string) => void store.delete(k),
            clear: () => store.clear()
        },
        configurable: true,
        writable: true
    })
}

function pressSave(view: EditorView) {
    view.contentDOM.dispatchEvent(
        new KeyboardEvent("keydown", { key: "s", code: "KeyS", ctrlKey: true, bubbles: true, cancelable: true })
    )
}

beforeEach(() => {
    installLocalStorage()
    lspExtensionsForFile.mockImplementation(async (_path: string, grade: string) =>
        grade === "full" ? { managed, extensions: [] } : null
    )
    clientRequest.mockResolvedValue([])
    getDocument.mockResolvedValue({ result: { kind: "full", content: "const x = 1\n", size: 12 } })
    useWorkspaceStore.setState({ workspacePath: "/w", pendingReveal: null })
})

afterEach(() => {
    cleanup()
    vi.clearAllMocks()
    useWorkspaceStore.setState({ pendingReveal: null, workspacePath: null })
})

describe("EditorPane LSP integration", () => {
    it("delegates the LSP mount to lspExtensionsForFile with the full grade for a .ts file", async () => {
        render(<EditorPane path={PATH} />)
        await waitFor(() => expect(lspExtensionsForFile).toHaveBeenCalledWith(PATH, "full"))
    })

    it("passes the very-long-line grade so lspExtensionsForFile refuses the server", async () => {
        getDocument.mockResolvedValue({
            result: { kind: "full", content: "x".repeat(10_001) + "\n", size: 10_002 }
        })
        render(<EditorPane path={PATH} />)
        await waitFor(() => expect(lspExtensionsForFile).toHaveBeenCalledWith(PATH, "veryLongLine"))
    })

    it("passes the limited grade so lspExtensionsForFile refuses the server", async () => {
        getDocument.mockResolvedValue({ result: { kind: "limited", content: "const x = 1\n", size: 12 } })
        render(<EditorPane path={PATH} />)
        await waitFor(() => expect(lspExtensionsForFile).toHaveBeenCalledWith(PATH, "limited"))
    })

    it("keeps registering the view (existing timing not broken by the async mount)", async () => {
        render(<EditorPane path={PATH} />)
        await waitFor(() => expect(registerView).toHaveBeenCalledWith(PATH, expect.anything()))
        await waitFor(() => expect(lspExtensionsForFile).toHaveBeenCalled())
        expect(registerView).toHaveBeenCalledTimes(1)
    })

    it("focuses the view when a reveal navigation lands on the live pane", async () => {
        render(<EditorPane path={PATH} />)
        await waitFor(() => expect(registerView).toHaveBeenCalled())
        const view = registerView.mock.calls[0][1] as EditorView
        const focusSpy = vi.spyOn(view, "focus")
        act(() => {
            useWorkspaceStore.getState().requestReveal(PATH, 2)
        })
        expect(focusSpy).toHaveBeenCalled()
    })

    it("does not focus the view for a reveal that opts out of focus (search click, A4)", async () => {
        render(<EditorPane path={PATH} />)
        await waitFor(() => expect(registerView).toHaveBeenCalled())
        const view = registerView.mock.calls[0][1] as EditorView
        const focusSpy = vi.spyOn(view, "focus")
        act(() => {
            useWorkspaceStore.getState().requestReveal(PATH, 2, false)
        })
        expect(focusSpy).not.toHaveBeenCalled()
    })

    it("intercepts clicks on links inside hover tooltips to block SPA navigation escape (H3)", async () => {
        render(<EditorPane path={PATH} />)
        await waitFor(() => expect(registerView).toHaveBeenCalled())

        const tooltip = document.createElement("div")
        tooltip.className = "cm-tooltip"
        const link = document.createElement("a")
        link.href = "https://evil.example/x"
        tooltip.appendChild(link)
        document.body.appendChild(tooltip)

        const evt = new MouseEvent("click", { bubbles: true, cancelable: true })
        link.dispatchEvent(evt)
        expect(evt.defaultPrevented).toBe(true)

        // A link OUTSIDE a tooltip is left alone.
        const plain = document.createElement("a")
        plain.href = "https://ok.example"
        document.body.appendChild(plain)
        const evt2 = new MouseEvent("click", { bubbles: true, cancelable: true })
        plain.dispatchEvent(evt2)
        expect(evt2.defaultPrevented).toBe(false)

        document.body.removeChild(tooltip)
        document.body.removeChild(plain)
    })

    it("flushes pending changes and requests formatting on save when enabled", async () => {
        localStorage.setItem(FORMAT_ON_SAVE_STORAGE_KEY, "true")
        render(<EditorPane path={PATH} />)
        await waitFor(() => expect(lspExtensionsForFile).toHaveBeenCalled())
        await flushMount()
        const view = registerView.mock.calls[0][1] as EditorView
        pressSave(view)
        await waitFor(() => expect(clientSync).toHaveBeenCalled())
        await waitFor(() =>
            expect(clientRequest).toHaveBeenCalledWith("textDocument/formatting", expect.anything())
        )
        await waitFor(() => expect(saveFile).toHaveBeenCalled())
    })

    it("does not request formatting on save when the setting is off", async () => {
        render(<EditorPane path={PATH} />)
        await waitFor(() => expect(lspExtensionsForFile).toHaveBeenCalled())
        await flushMount()
        const view = registerView.mock.calls[0][1] as EditorView
        pressSave(view)
        await waitFor(() => expect(clientSync).toHaveBeenCalled())
        await waitFor(() => expect(saveFile).toHaveBeenCalled())
        expect(clientRequest).not.toHaveBeenCalled()
    })

    it("applies real formatting edits into the doc and saves the formatted content (coverage gap)", async () => {
        localStorage.setItem(FORMAT_ON_SAVE_STORAGE_KEY, "true")
        clientRequest.mockResolvedValue([
            { range: { start: { line: 0, character: 0 }, end: { line: 0, character: 11 } }, newText: "const x = 1;" }
        ])
        render(<EditorPane path={PATH} />)
        await waitFor(() => expect(lspExtensionsForFile).toHaveBeenCalled())
        await flushMount()
        const view = registerView.mock.calls[0][1] as EditorView
        pressSave(view)
        await waitFor(() => expect(saveFile).toHaveBeenCalled())
        expect(view.state.doc.toString()).toBe("const x = 1;\n")
        expect(saveFile).toHaveBeenCalledWith(PATH, "const x = 1;\n")
    })

    it("saves the un-formatted text when a malformed server returns an inverted range, rather than dropping the save (R2-2)", async () => {
        localStorage.setItem(FORMAT_ON_SAVE_STORAGE_KEY, "true")
        // start (line 2) clamps to a larger offset than end (line 0), so the edit's
        // from > to. Dispatching it throws "Invalid change range", which would reject
        // the save chain and silently drop the write — it must be skipped instead and
        // the un-formatted content saved.
        clientRequest.mockResolvedValue([
            { range: { start: { line: 2, character: 0 }, end: { line: 0, character: 0 } }, newText: "REFORMATTED" }
        ])
        render(<EditorPane path={PATH} />)
        await waitFor(() => expect(lspExtensionsForFile).toHaveBeenCalled())
        await flushMount()
        const view = registerView.mock.calls[0][1] as EditorView
        pressSave(view)
        await waitFor(() => expect(saveFile).toHaveBeenCalled())
        expect(view.state.doc.toString()).toBe("const x = 1\n")
        expect(saveFile).toHaveBeenCalledWith(PATH, "const x = 1\n")
    })

    it("skips a stale formatting edit when the user types during the request, saving the user's content (F4)", async () => {
        localStorage.setItem(FORMAT_ON_SAVE_STORAGE_KEY, "true")
        const deferred = makeDeferred<unknown>()
        clientRequest.mockReturnValue(deferred.promise)
        render(<EditorPane path={PATH} />)
        await waitFor(() => expect(lspExtensionsForFile).toHaveBeenCalled())
        await flushMount()
        const view = registerView.mock.calls[0][1] as EditorView

        pressSave(view)
        await waitFor(() =>
            expect(clientRequest).toHaveBeenCalledWith("textDocument/formatting", expect.anything())
        )
        // User types while the format request is in flight.
        act(() => {
            view.dispatch({ changes: { from: view.state.doc.length, insert: "// typed\n" } })
        })
        const afterEdit = view.state.doc.toString()
        // Server replies with an edit computed against the pre-edit doc; applying it
        // now would corrupt the just-typed content, so it must be dropped.
        deferred.resolve([
            { range: { start: { line: 0, character: 0 }, end: { line: 0, character: 11 } }, newText: "REFORMATTED" }
        ])
        await waitFor(() => expect(saveFile).toHaveBeenCalled())
        expect(view.state.doc.toString()).toBe(afterEdit)
        expect(saveFile).toHaveBeenCalledWith(PATH, afterEdit)
    })

    it("orders save steps: client sync (flush) then the formatting request then saveFile (A2)", async () => {
        localStorage.setItem(FORMAT_ON_SAVE_STORAGE_KEY, "true")
        const order: string[] = []
        clientSync.mockImplementation(() => void order.push("sync"))
        clientRequest.mockImplementation(async () => {
            order.push("format")
            return []
        })
        saveFile.mockImplementation(async () => {
            order.push("save")
            return 0
        })
        render(<EditorPane path={PATH} />)
        await waitFor(() => expect(lspExtensionsForFile).toHaveBeenCalled())
        await flushMount()
        const view = registerView.mock.calls[0][1] as EditorView
        pressSave(view)
        await waitFor(() => expect(order).toEqual(["sync", "format", "save"]))
    })
})

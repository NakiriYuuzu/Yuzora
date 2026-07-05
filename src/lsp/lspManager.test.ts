import { it, expect, vi, beforeEach, afterEach } from "vitest"

const lspStart = vi.fn()
const lspSend = vi.fn()
const lspStopWorkspace = vi.fn()
const openFile = vi.fn()
const saveFile = vi.fn()

vi.mock("../lib/ipc", () => ({
    lspStart: (...a: unknown[]) => lspStart(...a),
    lspSend: (...a: unknown[]) => lspSend(...a),
    lspStopWorkspace: (...a: unknown[]) => lspStopWorkspace(...a),
    openFile: (...a: unknown[]) => openFile(...a),
    saveFile: (...a: unknown[]) => saveFile(...a)
}))

import {
    ensureClient,
    stopWorkspace,
    lspExtensionsForFile,
    shouldFormatOnSave,
    flushPendingChanges,
    sanitizeHtml
} from "./lspManager"
import { useWorkspaceStore } from "../state/workspaceStore"
import { useLspStore } from "../state/lspStore"

function deferred<T = unknown>() {
    let resolve!: (v: T) => void
    const promise = new Promise<T>((r) => {
        resolve = r
    })
    return { promise, resolve }
}

// Drives the LSP initialize handshake to completion: finds the initialize
// request the client sent (captured by the lspSend mock), then replies with a
// matching id through the transport's onMessage dispatch (the 3rd arg the client
// received via lspStart), which resolves client.initializing.
function completeHandshake() {
    const initCall = lspSend.mock.calls.find((c) => {
        try {
            return JSON.parse(c[2] as string).method === "initialize"
        } catch {
            return false
        }
    })
    const id = JSON.parse(initCall![2] as string).id
    const dispatch = lspStart.mock.calls.at(-1)![2] as (m: string) => void
    dispatch(JSON.stringify({ jsonrpc: "2.0", id, result: { capabilities: {} } }))
}

function startInfo() {
    return {
        workspace: "/ws",
        language: "typescript",
        serverId: "ts",
        command: "tsserver",
        path: null,
        status: { status: "starting" },
        lastStartupLog: null,
        lastError: null,
        restartCount: 0
    }
}

beforeEach(() => {
    lspStart.mockReset().mockResolvedValue(startInfo())
    lspSend.mockReset().mockResolvedValue(undefined)
    lspStopWorkspace.mockReset().mockResolvedValue(undefined)
    openFile.mockReset()
    saveFile.mockReset().mockResolvedValue(0)
    useWorkspaceStore.setState({ workspacePath: "/ws" })
    useLspStore.getState().reset()
})

afterEach(() => {
    stopWorkspace("/ws")
})

it("lspExtensionsForFile returns null for non-full grades and never starts a server", async () => {
    expect(await lspExtensionsForFile("/ws/a.ts", "limited")).toBeNull()
    expect(await lspExtensionsForFile("/ws/a.ts", "tooLarge")).toBeNull()
    expect(await lspExtensionsForFile("/ws/a.ts", "veryLongLine")).toBeNull()
    expect(lspStart).not.toHaveBeenCalled()
})

it("lspExtensionsForFile returns {managed, extensions} for a full-grade file, and waits for the handshake first (F3 + A0)", async () => {
    let settled = false
    const p = lspExtensionsForFile("/ws/a.ts", "full").then((r) => {
        settled = true
        return r
    })
    // ensureClient (inside) connects the client and sends initialize. Until the
    // handshake completes, lspExtensionsForFile must NOT resolve — otherwise
    // assembleLspExtensions would gate on null serverCapabilities (A0 race).
    await vi.waitFor(() =>
        expect(
            lspSend.mock.calls.some((c) => {
                try {
                    return JSON.parse(c[2] as string).method === "initialize"
                } catch {
                    return false
                }
            })
        ).toBe(true)
    )
    await new Promise((r) => setTimeout(r, 20))
    expect(settled).toBe(false)

    completeHandshake()
    const result = await p
    expect(settled).toBe(true)
    expect(result).not.toBeNull()
    // The new shape: the ManagedClient (for save flush + format gating) plus the
    // assembled editor extensions, both derived after capabilities are ready.
    expect(result!.managed.language).toBe("typescript")
    expect(result!.managed.client.serverCapabilities).not.toBeNull()
    expect(result!.extensions).toBeTruthy()
})

it("wires the serverDiagnostics push channel so the client advertises publishDiagnostics support (F2)", async () => {
    lspStart.mockReset().mockResolvedValue(startInfo())
    lspSend.mockReset().mockResolvedValue(undefined)
    useWorkspaceStore.setState({ workspacePath: "/ws" })

    const managed = await ensureClient("/ws", "typescript")
    expect(managed).not.toBeNull()

    const initCall = lspSend.mock.calls.find((c) => {
        try {
            return JSON.parse(c[2] as string).method === "initialize"
        } catch {
            return false
        }
    })
    expect(initCall).toBeDefined()
    // serverDiagnostics()'s clientCapabilities merge into the initialize request;
    // the base client never advertises publishDiagnostics, so its presence proves
    // the push channel is wired onto the client.
    const caps = JSON.parse(initCall![2] as string).params.capabilities
    expect(caps.textDocument.publishDiagnostics?.versionSupport).toBe(true)
})

it("lspExtensionsForFile returns null for an unsupported file type", async () => {
    expect(await lspExtensionsForFile("/ws/a.txt", "full")).toBeNull()
    expect(lspStart).not.toHaveBeenCalled()
})

it("ensureClient returns one client per (ws,lang) and starts the server only once", async () => {
    lspStart.mockClear()
    const a = await ensureClient("/ws", "typescript")
    const b = await ensureClient("/ws", "typescript")
    expect(a).toBe(b)
    expect(lspStart).toHaveBeenCalledTimes(1)
})

it("does not connect a missing server, surfaces its status to the store, and yields no extensions (W3)", async () => {
    const missing = { ...startInfo(), status: { status: "missing", installHint: "install it" } }
    lspStart.mockResolvedValue(missing)

    const ext = await lspExtensionsForFile("/ws/a.ts", "full")

    expect(ext).toBeNull()
    // never sent the initialize request (which would go out via lspSend)
    expect(lspSend).not.toHaveBeenCalled()
    expect(useLspStore.getState().servers.typescript).toEqual(missing)
})

it("does not cache a crashed server, so a later ensureClient retries (W3)", async () => {
    lspStart.mockResolvedValueOnce({ ...startInfo(), status: { status: "crashed", reason: "boom" } })
    const first = await ensureClient("/ws", "typescript")
    expect(first).toBeNull()

    lspStart.mockResolvedValue(startInfo())
    const second = await ensureClient("/ws", "typescript")
    expect(second).not.toBeNull()
})

it("recovers from an lspStart rejection: ensureClient returns null, clears the cache, and retries (R3-3)", async () => {
    lspStart.mockReset().mockRejectedValueOnce(new Error("spawn failed"))

    const first = await ensureClient("/ws", "typescript")
    expect(first).toBeNull()

    // cache cleared (isCurrent guard) → a retry actually re-invokes lspStart
    lspStart.mockResolvedValue(startInfo())
    const second = await ensureClient("/ws", "typescript")
    expect(second).not.toBeNull()
    expect(lspStart).toHaveBeenCalledTimes(2)
})

it("stopWorkspace disconnects the client and calls lspStopWorkspace ipc once (R3-4)", async () => {
    const managed = await ensureClient("/ws", "typescript")
    expect(managed).not.toBeNull()
    const disconnect = vi.spyOn(managed!.client, "disconnect").mockImplementation(() => {})

    lspStopWorkspace.mockClear()
    stopWorkspace("/ws")

    expect(lspStopWorkspace).toHaveBeenCalledTimes(1)
    expect(lspStopWorkspace).toHaveBeenCalledWith("/ws")
    // disconnect is invoked via pending.then(...) — flush the microtask
    await vi.waitFor(() => expect(disconnect).toHaveBeenCalledTimes(1))
})

it("A→B→A: a superseded in-flight startup does not clobber the newer cached client (R2-4)", async () => {
    const d1 = deferred()
    const d2 = deferred()
    lspStart.mockReset()
    lspStart.mockReturnValueOnce(d1.promise).mockReturnValueOnce(d2.promise)

    const p1 = ensureClient("/ws", "typescript")
    stopWorkspace("/ws") // clears clients[key] while p1 is in-flight
    const p2 = ensureClient("/ws", "typescript")

    d1.resolve(startInfo())
    d2.resolve(startInfo())
    const r1 = await p1
    const r2 = await p2

    expect(r1).toBeNull() // superseded → bailed
    expect(r2).not.toBeNull() // newer session connected
    // p2 is still cached — p1's cleanup must not have deleted it
    const r3 = await ensureClient("/ws", "typescript")
    expect(r3).toBe(r2)
    expect(lspStart).toHaveBeenCalledTimes(2) // no third start
})

it("stopWorkspace during in-flight startup leaves nothing connected or cached (R2-8)", async () => {
    const d1 = deferred()
    lspStart.mockReset().mockReturnValueOnce(d1.promise)

    const p1 = ensureClient("/ws", "typescript")
    stopWorkspace("/ws")
    d1.resolve(startInfo())
    const r1 = await p1

    expect(r1).toBeNull() // superseded → bailed before connecting
    expect(lspSend).not.toHaveBeenCalled() // never sent initialize (no client)

    // nothing stale cached: a fresh ensureClient starts anew and connects
    lspStart.mockResolvedValue(startInfo())
    const r2 = await ensureClient("/ws", "typescript")
    expect(r2).not.toBeNull()
    expect(lspSend).toHaveBeenCalled()
})

it("does not write missing/crashed status for a workspace the UI has already left (R2-5)", async () => {
    const d1 = deferred()
    lspStart.mockReset().mockReturnValueOnce(d1.promise)
    useWorkspaceStore.setState({ workspacePath: "/ws" })

    const p1 = ensureClient("/ws", "typescript")
    // user switches workspace while the server is still starting
    useWorkspaceStore.setState({ workspacePath: "/other" })
    d1.resolve({ ...startInfo(), status: { status: "missing", installHint: "x" } })
    await p1

    expect(useLspStore.getState().servers.typescript).toBeUndefined()
})

it("marks the language initialized in the store once the handshake completes (R4-2)", async () => {
    lspStart.mockReset().mockResolvedValue(startInfo())
    lspSend.mockReset().mockResolvedValue(undefined)
    useWorkspaceStore.setState({ workspacePath: "/ws" })
    useLspStore.getState().reset()

    const managed = await ensureClient("/ws", "typescript")
    expect(managed).not.toBeNull()
    // not ready until the handshake actually resolves
    expect(useLspStore.getState().initialized.typescript).toBeUndefined()

    completeHandshake()

    await vi.waitFor(() => expect(useLspStore.getState().initialized.typescript).toBe(true))
})

it("does not mark initialized for a workspace the UI has left when the handshake completes (R4-2 guard)", async () => {
    lspStart.mockReset().mockResolvedValue(startInfo())
    lspSend.mockReset().mockResolvedValue(undefined)
    useWorkspaceStore.setState({ workspacePath: "/ws" })
    useLspStore.getState().reset()

    const managed = await ensureClient("/ws", "typescript")
    expect(managed).not.toBeNull()

    // user switches workspace before the handshake completes
    useWorkspaceStore.setState({ workspacePath: "/other" })
    completeHandshake()
    await new Promise((r) => setTimeout(r, 20))

    expect(useLspStore.getState().initialized.typescript).toBeUndefined()
})

it("stopWorkspace attaches a .catch to the lspStopWorkspace ipc call so a rejection can't go unhandled, and does not throw (R4-6)", () => {
    // Deterministic check: the ipc result must have .catch() invoked on it. A
    // real rejected promise would surface as an unhandled rejection only on a
    // later, worker-dependent tick — too flaky to assert — so we observe the
    // handler being attached directly.
    const catchSpy = vi.fn().mockReturnValue(undefined)
    lspStopWorkspace.mockReset().mockReturnValue({ catch: catchSpy })

    expect(() => stopWorkspace("/ws")).not.toThrow()

    expect(lspStopWorkspace).toHaveBeenCalledWith("/ws")
    expect(catchSpy).toHaveBeenCalledTimes(1)
})

it("records the healthy server's info in the store on the success path (R6-2)", async () => {
    lspStart.mockReset().mockResolvedValue(startInfo())
    useWorkspaceStore.setState({ workspacePath: "/ws" })
    useLspStore.getState().reset()

    const managed = await ensureClient("/ws", "typescript")
    expect(managed).not.toBeNull()

    expect(useLspStore.getState().servers.typescript).toBeDefined()
    expect(useLspStore.getState().servers.typescript.serverId).toBe("ts")
})

it("does not record server info for a workspace the UI has left (R6-2 guard)", async () => {
    const d1 = deferred()
    lspStart.mockReset().mockReturnValueOnce(d1.promise)
    useWorkspaceStore.setState({ workspacePath: "/ws" })
    useLspStore.getState().reset()

    const p1 = ensureClient("/ws", "typescript")
    useWorkspaceStore.setState({ workspacePath: "/other" }) // switched away mid-startup
    d1.resolve(startInfo())
    await p1

    expect(useLspStore.getState().servers.typescript).toBeUndefined()
})

it("shouldFormatOnSave gates on both the setting (A7 default OFF) and the server capability", () => {
    expect(shouldFormatOnSave({ documentFormattingProvider: true }, true)).toBe(true)
    expect(shouldFormatOnSave({ documentFormattingProvider: true }, false)).toBe(false)
    expect(shouldFormatOnSave({}, true)).toBe(false)
    expect(shouldFormatOnSave(null, true)).toBe(false)
})

it("flushPendingChanges delegates to the client's sync()", () => {
    const sync = vi.fn()
    flushPendingChanges({ client: { sync } as never, language: "typescript", capabilities: null })
    expect(sync).toHaveBeenCalledTimes(1)
})

it("sanitizeHtml strips scripts from server-provided markdown HTML (W9 XSS guard)", () => {
    const out = sanitizeHtml('<p>hi</p><script>alert(1)</script><img src=x onerror=alert(2)>')
    expect(out).not.toContain("<script")
    expect(out.toLowerCase()).not.toContain("onerror")
    expect(out).toContain("hi")
})

it("sanitizeHtml forbids form/map/area tags and target/usemap attrs, matching MarkdownPreview (R8-1)", () => {
    const form = sanitizeHtml('<form action="https://evil"><input><button>go</button><select></select><textarea></textarea></form>')
    expect(form).not.toContain("<form")
    expect(form).not.toContain("<input")
    expect(form).not.toContain("<button")
    expect(form).not.toContain("<select")
    expect(form).not.toContain("<textarea")

    const imgmap = sanitizeHtml('<img usemap="#m"><map name="m"><area href="https://evil"></map>')
    expect(imgmap).not.toContain("<map")
    expect(imgmap).not.toContain("<area")
    expect(imgmap.toLowerCase()).not.toContain("usemap")

    const link = sanitizeHtml('<a href="https://ok.com" target="_blank">x</a>')
    expect(link.toLowerCase()).not.toContain("target")

    // a <style> after other content would otherwise survive and apply CSS globally
    // to the whole webview (R9-2)
    const styleTag = sanitizeHtml('<p>doc</p><style>*{display:none}</style>')
    expect(styleTag).not.toContain("<style")

    const dialogTag = sanitizeHtml('<p>x</p><dialog open>hi</dialog>')
    expect(dialogTag).not.toContain("<dialog")

    // a style attribute would otherwise allow a fullscreen fixed overlay (fake UI)
    // or a background:url() tracking beacon inside a tooltip (R10-1)
    const styleAttr = sanitizeHtml('<div style="position:fixed;inset:0">x</div>')
    expect(styleAttr).not.toContain("style=")
    expect(styleAttr).not.toContain("position:fixed")

    // a class attribute would otherwise let global Tailwind overlay utilities
    // (fixed/inset-0/z-50/opacity-0) rebuild the fullscreen overlay (R11-1)
    const classAttr = sanitizeHtml('<div class="fixed inset-0 z-50">x</div>')
    expect(classAttr).not.toContain("class=")

    // legitimate markdown HTML is preserved
    const ok = sanitizeHtml('<p>see <a href="https://ok.com"><code>fn()</code></a></p>')
    expect(ok).toContain("<code>")
    expect(ok).toContain("href")
})

import { it, expect, vi, beforeEach, afterEach } from "vitest"
import { EditorState } from "@codemirror/state"
import { EditorView } from "@codemirror/view"

const openFile = vi.fn()
const saveFile = vi.fn()

vi.mock("../lib/ipc", () => ({
    openFile: (...a: unknown[]) => openFile(...a),
    saveFile: (...a: unknown[]) => saveFile(...a)
}))

import { YuzoraWorkspace, pathToUri, uriToPath } from "./workspace"
import { registerView, unregisterView } from "../editor/viewRegistry"
import { recentlySaved } from "../lib/saveSuppress"

function fakeClient() {
    return { didOpen: vi.fn(), didClose: vi.fn(), notification: vi.fn() }
}

beforeEach(() => {
    openFile.mockReset()
    saveFile.mockReset().mockResolvedValue(0)
})

afterEach(() => {
    vi.restoreAllMocks()
})

it.each([
    ["/workspace/hello world.ts", "file:///workspace/hello%20world.ts"],
    ["/workspace/中文檔名.ts", "file:///workspace/%E4%B8%AD%E6%96%87%E6%AA%94%E5%90%8D.ts"],
    ["C:\\Users\\Yuuzu\\hello world.ts", "file:///C:/Users/Yuuzu/hello%20world.ts"],
    ["\\\\?\\C:\\Users\\Yuuzu\\hello world.ts", "file:///C:/Users/Yuuzu/hello%20world.ts"],
    ["c:/Users/Yuuzu/中文.ts", "file:///C:/Users/Yuuzu/%E4%B8%AD%E6%96%87.ts"],
    ["\\\\server\\share\\a b.ts", "file://server/share/a%20b.ts"],
    ["\\\\?\\UNC\\server\\share\\a b.ts", "file://server/share/a%20b.ts"],
])("encodes a cross-platform file path as a valid file URI: %s", (path, uri) => {
    expect(pathToUri(path)).toBe(uri)
})

it.each([
    ["file:///workspace/hello%20world.ts", "/workspace/hello world.ts"],
    ["file:///C:/Users/Yuuzu/hello%20world.ts", "C:/Users/Yuuzu/hello world.ts"],
    ["file://server/share/a%20b.ts", "//server/share/a b.ts"],
])("decodes valid file URIs without losing drive or UNC authority: %s", (uri, path) => {
    expect(uriToPath(uri)).toBe(path)
})

it("background-loads an unopened file via openFile ipc and sends didOpen", async () => {
    openFile.mockResolvedValue({ kind: "full", content: "hello\nworld", size: 11 })
    const client = fakeClient()
    const ws = new YuzoraWorkspace(client as never, "typescript")

    const file = await ws.requestFile(pathToUri("/ws/bg.ts"))

    expect(openFile).toHaveBeenCalledWith("/ws/bg.ts")
    expect(client.didOpen).toHaveBeenCalledTimes(1)
    expect(file).not.toBeNull()
    expect(file!.uri).toBe(pathToUri("/ws/bg.ts"))
})

it("dedups concurrent requestFile for the same uri: one openFile, one didOpen, one entry (W1)", async () => {
    let resolveOpen: (v: unknown) => void = () => {}
    openFile.mockImplementation(
        () =>
            new Promise((r) => {
                resolveOpen = r
            })
    )
    const client = fakeClient()
    const ws = new YuzoraWorkspace(client as never, "typescript")
    const uri = pathToUri("/ws/bg.ts")

    const all = Promise.all([ws.requestFile(uri), ws.requestFile(uri), ws.requestFile(uri)])
    resolveOpen({ kind: "full", content: "x", size: 1 })
    await all

    expect(openFile).toHaveBeenCalledTimes(1)
    expect(client.didOpen).toHaveBeenCalledTimes(1)
    expect(ws.files.length).toBe(1)
})

it("seeds the background file version via nextFileVersion so didOpen=v0 and the first didChange=v1 (W2 monotonic)", async () => {
    openFile.mockResolvedValue({ kind: "full", content: "let foo = 1", size: 11 })
    const client = fakeClient()
    const ws = new YuzoraWorkspace(client as never, "typescript")
    const uri = pathToUri("/ws/bg.ts")

    await ws.requestFile(uri)
    const opened = client.didOpen.mock.calls[0][0]
    expect(opened.version).toBe(0)

    ws.updateFile(uri, { changes: [{ from: 0, to: 0, insert: "x" }] })
    const change = client.notification.mock.calls.find((c) => c[0] === "textDocument/didChange")!
    expect(change[1].textDocument.version).toBe(1)
})

it("background writeback also notifies the server via a full-document didChange (W6 resync)", async () => {
    openFile.mockResolvedValue({ kind: "full", content: "let foo = 1", size: 11 })
    const client = fakeClient()
    const ws = new YuzoraWorkspace(client as never, "typescript")
    const uri = pathToUri("/ws/bg.ts")

    await ws.requestFile(uri)
    ws.updateFile(uri, { changes: [{ from: 4, to: 7, insert: "bar" }] })

    const change = client.notification.mock.calls.find((c) => c[0] === "textDocument/didChange")
    expect(change).toBeTruthy()
    expect(change![1].textDocument.uri).toBe(uri)
    expect(change![1].textDocument.version).toBeGreaterThan(0)
    expect(change![1].contentChanges[0].text).toBe("let bar = 1")
})

it("background writeback attaches a .catch to saveFile so a disk failure can't go unhandled, and does not throw (R6-1)", async () => {
    openFile.mockResolvedValue({ kind: "full", content: "let foo = 1", size: 11 })
    // Deterministic check (like R4-6): the fire-and-forget saveFile result must
    // have .catch() invoked on it. A real rejected promise would surface as an
    // unhandled rejection only on a flaky later tick, so observe the handler.
    const catchSpy = vi.fn().mockReturnValue(undefined)
    saveFile.mockReset().mockReturnValue({ catch: catchSpy })
    const client = fakeClient()
    const ws = new YuzoraWorkspace(client as never, "typescript")
    const uri = pathToUri("/ws/bg.ts")
    await ws.requestFile(uri)

    expect(() => ws.updateFile(uri, { changes: [{ from: 4, to: 7, insert: "bar" }] })).not.toThrow()

    expect(saveFile).toHaveBeenCalledWith("/ws/bg.ts", "let bar = 1")
    expect(catchSpy).toHaveBeenCalledTimes(1)
})

it("degrades an openFile failure to null without rejecting, clears in-flight, and can retry (R2-1)", async () => {
    openFile.mockRejectedValueOnce(new Error("ENOENT"))
    const client = fakeClient()
    const ws = new YuzoraWorkspace(client as never, "typescript")
    const uri = pathToUri("/ws/gone.ts")

    await expect(ws.requestFile(uri)).resolves.toBeNull()
    expect(client.didOpen).not.toHaveBeenCalled()

    // in-flight entry cleared → a retry actually re-reads the file
    openFile.mockResolvedValueOnce({ kind: "full", content: "x", size: 1 })
    const file = await ws.requestFile(uri)
    expect(file).not.toBeNull()
    expect(openFile).toHaveBeenCalledTimes(2)
})

it("does not reject on a malformed (unencoded bare %) URI — degrades to null like any load failure (R7-1)", async () => {
    // Server may return an unencoded file URI with a bare '%'. uriToPath must stay
    // total so requestFile never rejects (which would wipe a whole find-references
    // Promise.all fan-out + leak an unhandled rejection).
    openFile.mockRejectedValue(new Error("ENOENT"))
    const client = fakeClient()
    const ws = new YuzoraWorkspace(client as never, "typescript")

    await expect(ws.requestFile("file:///ws/100%done.md")).resolves.toBeNull()
    expect(client.didOpen).not.toHaveBeenCalled()
    // Pin the fallback value: decode failure keeps the raw de-scheme'd body, not ""
    // or some other placeholder (R8-2 mutation guard).
    expect(openFile).toHaveBeenCalledWith("/ws/100%done.md")
})

it("decodes a legal percent-encoded URI unchanged (R7-1 regression guard)", async () => {
    openFile.mockResolvedValue({ kind: "full", content: "hi", size: 2 })
    const client = fakeClient()
    const ws = new YuzoraWorkspace(client as never, "typescript")

    const file = await ws.requestFile("file:///ws/%E4%BD%A0.md")

    expect(openFile).toHaveBeenCalledWith("/ws/你.md")
    expect(file).not.toBeNull()
})

it("does NOT background-load a non-full grade file (no didOpen)", async () => {
    openFile.mockResolvedValue({ kind: "limited", content: "big", size: 999 })
    const client = fakeClient()
    const ws = new YuzoraWorkspace(client as never, "typescript")

    const file = await ws.requestFile(pathToUri("/ws/big.ts"))

    expect(file).toBeNull()
    expect(client.didOpen).not.toHaveBeenCalled()
})

it("does NOT background-load a full file whose content has a very long line (R3-2 veryLongLine guard)", async () => {
    // kind:"full" but a single line > MAX_LINE_LEN_SYNTAX_OFF (10_000) → fileGradeOf
    // downgrades to "veryLongLine", so the background-load guard must reject it.
    const longLine = "x".repeat(10_001)
    openFile.mockResolvedValue({ kind: "full", content: longLine, size: longLine.length })
    const client = fakeClient()
    const ws = new YuzoraWorkspace(client as never, "typescript")

    const file = await ws.requestFile(pathToUri("/ws/long.ts"))

    expect(file).toBeNull()
    expect(client.didOpen).not.toHaveBeenCalled()
})

it("rename on an unopened file applies the edit to the background doc and writes it back to disk, marking recentlySaved first", async () => {
    openFile.mockResolvedValue({ kind: "full", content: "let foo = 1", size: 11 })
    const markSpy = vi.spyOn(recentlySaved, "mark")
    const client = fakeClient()
    const ws = new YuzoraWorkspace(client as never, "typescript")
    const uri = pathToUri("/ws/bg.ts")

    await ws.requestFile(uri)
    // rename foo -> bar : replace offsets [4,7)
    ws.updateFile(uri, { changes: [{ from: 4, to: 7, insert: "bar" }] })

    expect(markSpy).toHaveBeenCalledWith("/ws/bg.ts")
    expect(saveFile).toHaveBeenCalledWith("/ws/bg.ts", "let bar = 1")
    // mark must happen before the disk write
    expect(markSpy.mock.invocationCallOrder[0]).toBeLessThan(saveFile.mock.invocationCallOrder[0])
})

it("does NOT write back to disk when the target file is open in a tab: it dispatches into that view (single source of truth)", async () => {
    openFile.mockResolvedValue({ kind: "full", content: "let foo = 1", size: 11 })
    const client = fakeClient()
    const ws = new YuzoraWorkspace(client as never, "typescript")
    const uri = pathToUri("/ws/open.ts")

    await ws.requestFile(uri)
    const view = new EditorView({ state: EditorState.create({ doc: "let foo = 1" }) })
    registerView("/ws/open.ts", view)

    ws.updateFile(uri, { changes: [{ from: 4, to: 7, insert: "bar" }] })

    expect(saveFile).not.toHaveBeenCalled()
    expect(view.state.doc.toString()).toBe("let bar = 1")

    unregisterView("/ws/open.ts", view)
    view.destroy()
})

it("evicts the least-recently-used background file past the LRU limit, sending didClose", async () => {
    openFile.mockImplementation(async () => ({ kind: "full", content: "x", size: 1 }))
    const client = fakeClient()
    const ws = new YuzoraWorkspace(client as never, "typescript")

    // LRU limit is 20; the 21st insert evicts f0.
    for (let i = 0; i < 21; i++) {
        await ws.requestFile(pathToUri(`/ws/f${i}.ts`))
    }

    expect(client.didOpen).toHaveBeenCalledTimes(21)
    expect(client.didClose).toHaveBeenCalledWith(pathToUri("/ws/f0.ts"))
})

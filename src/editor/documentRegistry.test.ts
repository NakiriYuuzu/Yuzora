import { expect, test, afterEach, vi } from "vitest"
import { createElement } from "react"
import { act, render } from "@testing-library/react"
import * as events from "@tauri-apps/api/event"
import { mockIPC, clearMocks } from "@tauri-apps/api/mocks"
import {
    getDocument,
    saveDocumentContent,
    documentChangedOnDisk,
    sameDiskSnapshot,
    updateBuffer,
    dropDocument,
    renameDocument,
    reloadDocument,
    documentGeneration,
    clearAll
} from "./documentRegistry"
import { useWorkspaceStore } from "../state/workspaceStore"
import type { OpenFileResult } from "../lib/types"
import { reconnectRemoteWorkspaces, registerRuntimeWorkspace, saveRemoteFile } from "../lib/remoteFiles"
import * as ipc from "../lib/ipc"
import { remoteFilePath } from "../lib/runtimeIdentity"
import { ExternalChangeBridge } from "../workbench/ExternalChangeBridge"

vi.mock("@tauri-apps/api/event", async importOriginal => ({
    ...await importOriginal<typeof events>(),
    listen: vi.fn()
}))

afterEach(() => { vi.restoreAllMocks(); clearMocks(); clearAll(); useWorkspaceStore.setState({ workspacePath: null }) })

test.each([1, 2, 3, 4, 5])("releases document generation paths across 100 workspace lifecycles (run %i)", async run => {
    const prefix = `["/perf-gen-${run}-`
    const originalSet = Map.prototype.set
    let retainedSize = () => 0
    vi.spyOn(Map.prototype, "set").mockImplementation(function (this: Map<unknown, unknown>, key, value) {
        if (typeof key === "string" && key.startsWith(prefix) && typeof value === "number") {
            retainedSize = () => [...this.keys()].filter(candidate => typeof candidate === "string" && candidate.startsWith(prefix)).length
        }
        return originalSet.call(this, key, value)
    })
    mockIPC(() => ({ kind: "full", content: "disk", size: 4, lineEnding: "lf" }))
    const cycle = async (index: number) => {
        const workspace = `/perf-gen-${run}-${index}`
        useWorkspaceStore.setState({ workspacePath: workspace })
        const paths = [0, 1, 2].map(file => `${workspace}/file-${file}.txt`)
        for (const path of paths) await getDocument(path)
        await reloadDocument(paths[0])
        await reloadDocument(paths[0])
        const old = paths.map(path => documentGeneration(path))
        clearAll()
        for (let file = 0; file < paths.length; file++) {
            const entry = await getDocument(paths[file])
            expect(documentGeneration(paths[file])).not.toBe(old[file])
            updateBuffer(paths[file], "stale flush", old[file], workspace)
            expect(entry.result).toMatchObject({ content: "disk" })
            dropDocument(paths[file])
        }
    }
    for (let index = 0; index < 10; index++) await cycle(index)
    const afterWarmup = retainedSize()
    for (let index = 10; index < 110; index++) await cycle(index)
    const afterCycles = retainedSize()
    if (import.meta.env.YUZORA_PERF_MEASURE) console.info(JSON.stringify({ experiment: "document-generations", run, cycles: 100,
        afterWarmup, afterCycles, retainedPerCycle: (afterCycles - afterWarmup) / 100 }))
    expect(afterCycles).toBe(0)
})

test("clearAll invalidates a dropped document token without changing per-document reload increments", async () => {
    useWorkspaceStore.setState({ workspacePath: "/epoch-fixture" })
    mockIPC(() => ({ kind: "full", content: "disk", size: 4, lineEnding: "lf" }))
    const first = "/epoch-fixture/first.txt", second = "/epoch-fixture/second.txt"
    await getDocument(first)
    await getDocument(second)
    const secondBefore = documentGeneration(second)
    for (let i = 0; i < 3; i++) await reloadDocument(first)
    await reloadDocument(second)
    expect(documentGeneration(second)).toBe(secondBefore + 1)
    const stale = documentGeneration(first)
    dropDocument(first)
    clearAll()
    const reopened = await getDocument(first)
    updateBuffer(first, "stale flush", stale)
    expect(reopened.result).toMatchObject({ content: "disk" })
    expect(documentGeneration(first)).toBeGreaterThan(stale)
})

test("a failed save leaves the last-loaded disk snapshot intact", async () => {
    const path = "/w/failed-save.txt"
    const generation = documentGeneration(path)
    mockIPC(command => {
        if (command === "save_file") throw new Error("failed")
        return { kind: "full", content: "disk", size: 4, lineEnding: "lf" }
    })
    await getDocument(path)
    await expect(saveDocumentContent(path, "unsaved")).rejects.toThrow("failed")
    expect(await documentChangedOnDisk(path)).toBe(false)
    expect(documentGeneration(path)).toBe(generation)
})

test("same-path saves serialize writes and baseline updates despite reverse-ready replies", async () => {
    const path = "/w/ordered-save.txt"
    let disk = "A"
    const writes: string[] = []
    let finishB!: (value: number) => void
    let finishC!: (value: number) => void
    const replyB = new Promise<number>(resolve => { finishB = resolve })
    const replyC = new Promise<number>(resolve => { finishC = resolve })
    mockIPC((command, args) => {
        if (command === "save_file") {
            disk = (args as { content: string }).content
            writes.push(disk)
            return disk === "B" ? replyB : replyC
        }
        return { kind: "full", content: disk, size: 1, lineEnding: "lf" }
    })
    const entry = await getDocument(path)
    const first = saveDocumentContent(path, "B")
    const second = saveDocumentContent(path, "C")
    // C's response is ready before B's, but its write must not start yet.
    finishC(3)
    await new Promise(resolve => setTimeout(resolve, 0))
    const writesBeforeBReply = [...writes]
    finishB(2)
    expect(await first).toBe(2)
    expect(await second).toBe(3)
    expect(entry.diskResult).toMatchObject({ content: "C" })
    expect(writesBeforeBReply).toEqual(["B"])
    expect(writes).toEqual(["B", "C"])
    expect(disk).toBe("C")
    expect(await documentChangedOnDisk(path)).toBe(false)
})

test("a rejected queued save propagates its error without blocking the next save or other paths", async () => {
    const path = "/w/queue-failure.txt"
    let reject!: (error: Error) => void
    const failed = new Promise<number>((_resolve, rejectPromise) => { reject = rejectPromise })
    const writes: string[] = []
    mockIPC((command, args) => {
        if (command === "save_file") {
            const { content } = args as { content: string }
            writes.push(content)
            return content === "B" ? failed : 3
        }
        return { kind: "full", content: "A", size: 1, lineEnding: "lf" }
    })
    const entry = await getDocument(path)
    const first = saveDocumentContent(path, "B")
    const rejection = expect(first).rejects.toThrow("disk full")
    const second = saveDocumentContent(path, "C")
    await saveDocumentContent("/w/independent.txt", "independent")
    expect(entry.diskResult).toMatchObject({ content: "A" })
    expect(writes).toEqual(["B", "independent"])
    reject(new Error("disk full"))
    await rejection
    expect(await second).toBe(3)
    expect(entry.diskResult).toMatchObject({ content: "C" })
    expect(writes).toEqual(["B", "independent", "C"])
})

test.each(["resolve", "reject"] as const)("remote queued saves retain G1 after reconnect when the first write replies with %s", async (reply) => {
    const owner = { hostId: `registry-save-reconnect-${reply}`, generation: 1 }
    const writes: { generation: number; content: string }[] = []
    let finish!: (value: { revision: string }) => void
    let reject!: (error: Error) => void
    const deferred = new Promise<{ revision: string }>((resolve, rejectPromise) => { finish = resolve; reject = rejectPromise })
    mockIPC((command, payload) => {
        if (command !== "host_request") return
        const { owner, operation } = payload as {
            owner: { generation: number }
            operation: { method: string; params: { content: string } }
        }
        if (operation.method === "workspaceOpen") return { canonicalPath: "/project", capabilityId: `workspace-${owner.generation}` }
        if (operation.method === "filesWrite") {
            writes.push({ generation: owner.generation, content: operation.params.content })
            return operation.params.content === "B" ? deferred : { revision: "saved" }
        }
        return { file: { kind: "full", content: "A", size: 1, lineEnding: "lf" }, revision: "original" }
    })
    const root = await registerRuntimeWorkspace(owner, "/project", () => true)
    useWorkspaceStore.setState({ workspacePath: root })
    const path = root + "/file.txt"
    const entry = await getDocument(path)
    const first = saveDocumentContent(path, "B")
    await vi.waitFor(() => expect(writes).toEqual([{ generation: 1, content: "B" }]))
    const second = saveDocumentContent(path, "C")
    const outcomes = Promise.allSettled([first, second])
    // Let saveFile's dynamic import enter the remote queue before reconnecting.
    await new Promise(resolve => setTimeout(resolve, 0))
    await reconnectRemoteWorkspaces({ ...owner, generation: 2 }, () => true)
    if (reply === "resolve") finish({ revision: "written-on-G1" })
    else reject(new Error("G1 write failed"))
    const [firstOutcome, secondOutcome] = await outcomes
    expect(firstOutcome.status).toBe("rejected")
    expect.soft(secondOutcome).toMatchObject({ status: "rejected", reason: expect.objectContaining({ message: expect.stringContaining("connection changed") }) })
    expect.soft(writes).toEqual([{ generation: 1, content: "B" }])
    expect.soft(entry.diskResult).toMatchObject({ content: "A" })
    // Only a new request made on G2 may write there; rejected tails cannot block it.
    expect(await saveDocumentContent(path, "D")).toBe(1)
    expect(writes).toEqual([{ generation: 1, content: "B" }, { generation: 2, content: "D" }])
    expect(entry.diskResult).toMatchObject({ content: "D" })
})

test("a successful G1 save remains the baseline after queued C is rejected on reconnect and flags an external revert", async () => {
    const owner = { hostId: "registry-save-success-reconnect", generation: 1 }
    const writes: { generation: number; content: string }[] = []
    let disk = "A"
    let revision = "original"
    let reconnect!: Promise<void>
    let finishB!: (value: { revision: string }) => void
    const replyB = new Promise<{ revision: string }>(resolve => { finishB = resolve })
    mockIPC((command, payload) => {
        if (command !== "host_request") return
        const { owner, operation } = payload as {
            owner: { generation: number }
            operation: { method: string; params: { content: string } }
        }
        if (operation.method === "workspaceOpen") return { canonicalPath: "/project", capabilityId: `workspace-${owner.generation}` }
        if (operation.method === "filesWrite") {
            writes.push({ generation: owner.generation, content: operation.params.content })
            disk = operation.params.content
            return disk === "B" ? replyB : { revision: "saved" }
        }
        return { file: { kind: "full", content: disk, size: 1, lineEnding: "lf" }, revision }
    })
    const root = await registerRuntimeWorkspace(owner, "/project", () => true)
    useWorkspaceStore.setState({ workspacePath: root })
    const path = root + "/file.txt"
    const entry = await getDocument(path)
    const first = saveDocumentContent(path, "B")
    await vi.waitFor(() => expect(writes).toEqual([{ generation: 1, content: "B" }]))
    const second = saveDocumentContent(path, "C")
    const outcomes = Promise.allSettled([first, second])
    // Both requests must enter the real provider on G1 before B replies.
    await new Promise(resolve => setTimeout(resolve, 0))
    finishB({
        get revision() {
            revision = "written-on-G1"
            // Start reconnect at B's response: its synchronous post-write guard
            // still sees G1; workspaceOpen switches to G2 during queue release,
            // before queued C can pass its own backend guard.
            reconnect = reconnectRemoteWorkspaces({ ...owner, generation: 2 }, () => true)
            return revision
        }
    })
    const [firstOutcome, secondOutcome] = await outcomes
    await reconnect
    expect(firstOutcome).toEqual({ status: "fulfilled", value: 1 })
    expect(secondOutcome).toMatchObject({ status: "rejected", reason: expect.objectContaining({ message: expect.stringContaining("connection changed") }) })
    expect(writes).toEqual([{ generation: 1, content: "B" }])
    expect(disk).toBe("B")
    expect.soft(entry.diskResult).toMatchObject({ content: "B" })

    // An external B -> A is a real conflict with the still-dirty C buffer.
    updateBuffer(path, "C", documentGeneration(path))
    useWorkspaceStore.getState().openTab(path)
    useWorkspaceStore.getState().markDirty(path, true)
    disk = "A"
    revision = "external-revert"
    expect.soft(await documentChangedOnDisk(path)).toBe(true)
    let listener!: (event: { payload: { workspaceRoot: string; paths: string[] } }) => void
    vi.mocked(events.listen).mockImplementation(async (_event, callback) => {
        listener = callback as typeof listener
        return () => {}
    })
    const bridge = render(createElement(ExternalChangeBridge))
    try {
        await act(async () => { listener({ payload: { workspaceRoot: root, paths: [path] } }) })
        const tab = useWorkspaceStore.getState().groups.flatMap(group => group.tabs).find(tab => tab.path === path)
        expect(tab).toMatchObject({ dirty: true, externallyModified: true })
        expect(entry.result).toMatchObject({ content: "C" })
    } finally {
        bridge.unmount()
        useWorkspaceStore.getState().closeTab(0, path)
    }
})

test.each(["resolve", "reject"] as const)("remote reverse-order replies preserve the latest successful baseline after C's %s", async (reply) => {
    const path = remoteFilePath(`registry-save-order-${reply}`, "/project/file.txt", "/project")
    const snapshot = { kind: "full", content: "A", size: 1, lineEnding: "lf" } as const
    vi.spyOn(ipc, "openFileSnapshot").mockResolvedValue({ result: snapshot, accept: () => {} })
    let finishB!: (value: number) => void
    let finishC!: (value: number) => void
    let rejectC!: (error: Error) => void
    const replyB = new Promise<number>(resolve => { finishB = resolve })
    const replyC = new Promise<number>((resolve, reject) => { finishC = resolve; rejectC = reject })
    // Isolate registry reply ordering from the remote provider's write queue.
    const save = vi.spyOn(ipc, "saveFile").mockReturnValueOnce(replyB).mockReturnValueOnce(replyC)
    const entry = await getDocument(path)
    const first = saveDocumentContent(path, "B")
    const second = saveDocumentContent(path, "C")
    const secondOutcome = second.then(value => value, error => error)
    const callsBeforeReplies = save.mock.calls.length
    if (reply === "resolve") finishC(3)
    else rejectC(new Error("C failed"))
    // With the old registry queue C cannot settle until B, so release both on failure.
    if (callsBeforeReplies !== 2) finishB(2)
    expect(await secondOutcome).toEqual(reply === "resolve" ? 3 : new Error("C failed"))
    expect(entry.diskResult).toMatchObject({ content: reply === "resolve" ? "C" : "A" })
    finishB(2)
    expect(await first).toBe(2)
    expect(entry.diskResult).toMatchObject({ content: reply === "resolve" ? "C" : "B" })
    expect(callsBeforeReplies).toBe(2)
})

test.each(["resolve", "reject"] as const)("a remote success advances the baseline while C is pending and preserves it after C's %s", async (reply) => {
    const path = remoteFilePath(`registry-save-latest-pending-${reply}`, "/project/file.txt", "/project")
    const snapshot = { kind: "full", content: "A", size: 1, lineEnding: "lf" } as const
    vi.spyOn(ipc, "openFileSnapshot").mockResolvedValue({ result: snapshot, accept: () => {} })
    let finishB!: (value: number) => void
    let finishC!: (value: number) => void
    let rejectC!: (error: Error) => void
    const replyB = new Promise<number>(resolve => { finishB = resolve })
    const replyC = new Promise<number>((resolve, reject) => { finishC = resolve; rejectC = reject })
    const save = vi.spyOn(ipc, "saveFile").mockReturnValueOnce(replyB).mockReturnValueOnce(replyC).mockResolvedValue(4)
    const entry = await getDocument(path)
    const first = saveDocumentContent(path, "B")
    const second = saveDocumentContent(path, "C")
    const secondOutcome = second.then(value => value, error => error)
    expect(save).toHaveBeenCalledTimes(2)
    finishB(2)
    expect(await first).toBe(2)
    const baselineAfterB = entry.diskResult
    if (reply === "resolve") finishC(3)
    else rejectC(new Error("C failed"))
    expect(await secondOutcome).toEqual(reply === "resolve" ? 3 : new Error("C failed"))
    expect.soft(baselineAfterB).toMatchObject({ content: "B" })
    expect.soft(entry.diskResult).toMatchObject({ content: reply === "resolve" ? "C" : "B" })
    expect(await saveDocumentContent(path, "D")).toBe(4)
    expect(entry.diskResult).toMatchObject({ content: "D" })
})

test("explicit reload replaces an unsaved buffer even when disk equals its baseline", async () => {
    const path = "/w/explicit-reload.txt"
    mockIPC(() => ({ kind: "full", content: "A", size: 1, lineEnding: "lf" }))
    const entry = await getDocument(path)
    const generation = documentGeneration(path)
    updateBuffer(path, "B", generation)
    expect(await reloadDocument(path, () => true, "reconcile")).toBe(entry)
    expect(documentGeneration(path)).toBe(generation)
    const reloaded = await reloadDocument(path)
    expect(reloaded.result).toMatchObject({ content: "A" })
    expect(reloaded).not.toBe(entry)
    expect(documentGeneration(path)).toBe(generation + 1)
})

test("disk snapshots distinguish kind, line endings, encoding and byte size", () => {
    const full = { kind: "full", content: "same", size: 4, lineEnding: "lf" } as const
    expect(sameDiskSnapshot(full, { ...full })).toBe(true)
    expect(sameDiskSnapshot(full, { ...full, kind: "limited" })).toBe(false)
    expect(sameDiskSnapshot(full, { ...full, lineEnding: "crlf" })).toBe(false)
    expect(sameDiskSnapshot(full, { ...full, size: 5 })).toBe(false)
    const encoded = { kind: "nonUtf8Readonly", content: "same", size: 8, encoding: "UTF-16LE" } as const
    expect(sameDiskSnapshot(encoded, { ...encoded, encoding: "UTF-16BE" })).toBe(false)
    expect(sameDiskSnapshot(encoded, { ...encoded })).toBe(true)
    expect(sameDiskSnapshot({ kind: "binary", size: 4 }, { kind: "binary", size: 4 })).toBe(false)
})

test("unsaved cached buffers and renames retain their separate disk snapshot", async () => {
    mockIPC(() => ({ kind: "full", content: "disk", size: 4, lineEnding: "lf" }))
    await getDocument("/w/old.txt")
    updateBuffer("/w/old.txt", "unsaved", documentGeneration("/w/old.txt"))
    renameDocument("/w/old.txt", "/w/new.txt", "live unsaved")
    expect(await documentChangedOnDisk("/w/new.txt")).toBe(false)
    expect((await getDocument("/w/new.txt")).result).toMatchObject({ content: "live unsaved" })
})

test("overlapping native workspaces isolate document content and late pane cleanup", async () => {
    const path = "/repo/nested/shared.ts"
    mockIPC(() => ({ kind: "full", content: "disk", size: 4 }))
    useWorkspaceStore.setState({ workspacePath: "/repo" })
    await getDocument(path)
    const previousGeneration = documentGeneration(path)
    updateBuffer(path, "parent edit", previousGeneration)
    useWorkspaceStore.setState({ workspacePath: "/repo/nested" })
    expect((await getDocument(path)).result).toMatchObject({ content: "disk" })
    updateBuffer(path, "late parent cleanup", previousGeneration, "/repo")
    expect((await getDocument(path)).result).toMatchObject({ content: "disk" })
    useWorkspaceStore.setState({ workspacePath: "/repo" })
    expect((await getDocument(path)).result).toMatchObject({ content: "parent edit" })
})

test("a pending read cannot repopulate a cleared workspace cache", async () => {
    let finish!: (value: OpenFileResult) => void
    mockIPC(() => new Promise<OpenFileResult>((resolve) => { finish = resolve }))
    const pending = getDocument("/repo/pending.ts")
    clearAll()
    finish({ kind: "full", content: "stale", size: 5, lineEnding: "lf" })
    await expect(pending).rejects.toThrow("workspace changed")
    mockIPC(() => ({ kind: "full", content: "new", size: 3 }))
    expect((await getDocument("/repo/pending.ts")).result).toMatchObject({ content: "new" })
})

test("a slow reload preserves edits made while the remote read is pending", async () => {
    const path = "/repo/slow.ts"
    mockIPC(() => ({ kind: "full", content: "before", size: 6, lineEnding: "lf" }))
    await getDocument(path)
    const generation = documentGeneration(path)
    let finish!: (value: OpenFileResult) => void
    mockIPC(() => new Promise<OpenFileResult>((resolve) => { finish = resolve }))
    const reload = reloadDocument(path)
    updateBuffer(path, "my new edits", generation)
    expect((await getDocument(path)).result).toMatchObject({ content: "my new edits" })
    finish({ kind: "full", content: "external", size: 8, lineEnding: "lf" })
    await expect(reload).rejects.toThrow("changed during reload")
    expect(documentGeneration(path)).toBe(generation)
    expect((await getDocument(path)).result).toMatchObject({ content: "my new edits" })
})

test("an older reload cannot replace a newer accepted document", async () => {
    const path = "/repo/order.ts"
    mockIPC(() => ({ kind: "full", content: "before", size: 6, lineEnding: "lf" }))
    await getDocument(path)
    let finish!: (value: OpenFileResult) => void
    mockIPC(() => new Promise<OpenFileResult>((resolve) => { finish = resolve }))
    const old = reloadDocument(path)
    mockIPC(() => ({ kind: "full", content: "newer", size: 5, lineEnding: "lf" }))
    await reloadDocument(path)
    finish({ kind: "full", content: "older", size: 5, lineEnding: "lf" })
    await expect(old).rejects.toThrow("changed during reload")
    expect((await getDocument(path)).result).toMatchObject({ content: "newer" })
})

test("competing remote reloads commit only the revision of the accepted buffer", async () => {
    const owner = { hostId: "registry-reload-race", generation: 1 }
    const pending: ((value: unknown) => void)[] = []
    let deferred = false
    let writtenRevision: unknown
    mockIPC((_command, payload) => {
        const { operation } = payload as { operation: { method: string; params: { revision?: string } } }
        if (operation.method === "workspaceOpen") return { canonicalPath: "/project", capabilityId: "workspace" }
        if (operation.method === "filesWrite") {
            writtenRevision = operation.params.revision
            return { revision: "saved" }
        }
        if (deferred) return new Promise((resolve) => pending.push(resolve))
        return { file: { kind: "full", content: "before", size: 6, lineEnding: "lf" }, revision: "before" }
    })
    const root = await registerRuntimeWorkspace(owner, "/project", () => true)
    useWorkspaceStore.setState({ workspacePath: root })
    const path = root + "/file.txt"
    await getDocument(path)
    deferred = true
    const first = reloadDocument(path)
    const second = reloadDocument(path)
    // The remote import and IPC entry are asynchronous.
    while (pending.length < 2) await new Promise((resolve) => setTimeout(resolve, 0))
    pending[0]({ file: { kind: "full", content: "accepted", size: 8, lineEnding: "lf" }, revision: "accepted-revision" })
    pending[1]({ file: { kind: "full", content: "discarded", size: 9, lineEnding: "lf" }, revision: "unseen-revision" })
    await first
    await expect(second).rejects.toThrow("changed during reload")
    expect((await getDocument(path)).result).toMatchObject({ content: "accepted" })
    await saveRemoteFile(path, "my edits")
    expect(writtenRevision).toBe("accepted-revision")
})

test("renameDocument 把快取移到新 key，新 path getDocument 命中快取（不再走 IPC），舊 path miss", async () => {
    let calls = 0
    mockIPC((cmd) => {
        if (cmd === "open_file") {
            calls++
            return { kind: "full", content: "disk", size: 4 }
        }
        return undefined
    })
    await getDocument("/w/old.ts")
    renameDocument("/w/old.ts", "/w/new.ts")
    const moved = await getDocument("/w/new.ts")
    expect(moved.result.kind).toBe("full")
    // new path served from the moved cache entry — no second open_file.
    expect(calls).toBe(1)
    // old path is now a miss → re-reads from disk.
    await getDocument("/w/old.ts")
    expect(calls).toBe(2)
})

test("renameDocument 帶 liveContent 時把未存檔內容一起帶到新 key（rename 保留未存編輯）", async () => {
    mockIPC((cmd) =>
        cmd === "open_file" ? { kind: "full", content: "saved", size: 5 } : undefined
    )
    await getDocument("/w/old.ts")
    renameDocument("/w/old.ts", "/w/new.ts", "unsaved-edit")
    const moved = await getDocument("/w/new.ts")
    expect(moved.result.kind).toBe("full")
    if (moved.result.kind === "full") expect(moved.result.content).toBe("unsaved-edit")
})

test("renameDocument 把 generation 一起移到新 path，後續 reload 仍能前進", async () => {
    const unmappedGeneration = documentGeneration("/w/gen-old.ts")
    let calls = 0
    mockIPC((cmd) => {
        if (cmd !== "open_file") return undefined
        calls++
        return { kind: "full", content: `disk-${calls}`, size: 6, lineEnding: "lf" }
    })
    await getDocument("/w/gen-old.ts")
    await reloadDocument("/w/gen-old.ts")
    const movedGeneration = documentGeneration("/w/gen-old.ts")

    renameDocument("/w/gen-old.ts", "/w/gen-new.ts")

    expect(documentGeneration("/w/gen-old.ts")).toBe(unmappedGeneration)
    expect(documentGeneration("/w/gen-new.ts")).toBe(movedGeneration)
    await reloadDocument("/w/gen-new.ts")
    expect(documentGeneration("/w/gen-new.ts")).toBe(movedGeneration + 1)
})

test("renameDocument 對未開啟的 path 為 no-op（不會憑空建立新 key）", async () => {
    let calls = 0
    mockIPC((cmd) => {
        if (cmd === "open_file") {
            calls++
            return { kind: "full", content: "disk", size: 4 }
        }
        return undefined
    })
    renameDocument("/w/never.ts", "/w/target.ts")
    // target was never populated → getDocument must go to IPC.
    await getDocument("/w/target.ts")
    expect(calls).toBe(1)
})

test("updateBuffer 寫回 buffer，再次 getDocument 回未存檔內容", async () => {
    mockIPC((cmd) =>
        cmd === "open_file" ? { kind: "full", content: "old", size: 3 } : undefined
    )
    await getDocument("/w/a.ts")
    updateBuffer("/w/a.ts", "new", documentGeneration("/w/a.ts"))
    const entry = await getDocument("/w/a.ts")
    expect(entry.result.kind).toBe("full")
    if (entry.result.kind === "full") expect(entry.result.content).toBe("new")
})

test("updateBuffer 對未開啟的 path 為 no-op", () => {
    expect(() => updateBuffer("/w/never-opened.ts", "x", 0)).not.toThrow()
})

test("dropDocument 後 getDocument 重新走 IPC", async () => {
    let calls = 0
    mockIPC((cmd) => {
        if (cmd === "open_file") {
            calls++
            return { kind: "full", content: "disk", size: 4 }
        }
        return undefined
    })
    await getDocument("/w/b.ts")
    dropDocument("/w/b.ts")
    await getDocument("/w/b.ts")
    expect(calls).toBe(2)
})

test("updateBuffer 帶舊 generation 在 reloadDocument 後為 no-op（防止舊 pane 的 stale flush 蓋掉剛 reload 的磁碟新內容）", async () => {
    let openCalls = 0
    mockIPC((cmd) => {
        if (cmd === "open_file") {
            openCalls++
            return openCalls === 1
                ? { kind: "full", content: "disk-v1", size: 7 }
                : { kind: "full", content: "disk-v2", size: 7 }
        }
        return undefined
    })
    await getDocument("/w/c.ts")
    // 模擬 EditorPane effect 掛載當下捕捉的 generation（此時尚未 reload）
    const staleGeneration = documentGeneration("/w/c.ts")
    // 外部檔案變更觸發 reload：registry 清空、重新從磁碟讀入 disk-v2、成功後才 generation bump
    await reloadDocument("/w/c.ts")
    // 舊 EditorPane 的 unmount cleanup 此時才執行，帶著掛載當下捕捉的舊 generation 嘗試 flush 舊 buffer
    updateBuffer("/w/c.ts", "stale-buffer-from-old-pane", staleGeneration)
    const entry = await getDocument("/w/c.ts")
    expect(entry.result.kind).toBe("full")
    if (entry.result.kind === "full") expect(entry.result.content).toBe("disk-v2")
})

test("updateBuffer 帶當前 generation 在 reloadDocument 後仍正常寫回（write-through 本體不退化）", async () => {
    let openCalls = 0
    mockIPC((cmd) => {
        if (cmd === "open_file") {
            openCalls++
            return openCalls === 1
                ? { kind: "full", content: "disk-v1", size: 7 }
                : { kind: "full", content: "disk-v2", size: 7 }
        }
        return undefined
    })
    await getDocument("/w/d.ts")
    await reloadDocument("/w/d.ts")
    const currentGeneration = documentGeneration("/w/d.ts")
    updateBuffer("/w/d.ts", "edited-after-reload", currentGeneration)
    const entry = await getDocument("/w/d.ts")
    expect(entry.result.kind).toBe("full")
    if (entry.result.kind === "full") expect(entry.result.content).toBe("edited-after-reload")
})

test("reloadDocument 成功後才 generation +1，且回傳磁碟新內容", async () => {
    let openCalls = 0
    mockIPC((cmd) => {
        if (cmd === "open_file") {
            openCalls++
            return openCalls === 1
                ? { kind: "full", content: "disk-v1", size: 7 }
                : { kind: "full", content: "disk-v2", size: 7 }
        }
        return undefined
    })
    await getDocument("/w/reload-ok.ts")
    const gen0 = documentGeneration("/w/reload-ok.ts")
    const entry = await reloadDocument("/w/reload-ok.ts")
    expect(documentGeneration("/w/reload-ok.ts")).toBe(gen0 + 1)
    expect(entry.result.kind).toBe("full")
    if (entry.result.kind === "full") expect(entry.result.content).toBe("disk-v2")
})

test("reloadDocument 失敗（檔案已刪）時 generation 不變且 rejection 傳出（呼叫端負責 catch）", async () => {
    let openCalls = 0
    mockIPC((cmd) => {
        if (cmd === "open_file") {
            openCalls++
            if (openCalls === 1) return { kind: "full", content: "disk", size: 4 }
            return Promise.reject(new Error("not found"))
        }
        return undefined
    })
    await getDocument("/w/reload-gone.ts")
    const gen0 = documentGeneration("/w/reload-gone.ts")
    // 重新 fetch 失敗時 generation 必須不變：否則 keyed EditorArea 會 remount，舊 pane
    // 的未存 buffer 被 gen-guard 的 updateBuffer no-op 抹掉（R3-F1 根因）。
    await expect(reloadDocument("/w/reload-gone.ts")).rejects.toThrow()
    expect(documentGeneration("/w/reload-gone.ts")).toBe(gen0)
})

test("dropping a file invalidates a pending read even when it is reopened in the same workspace", async () => {
    let finish!: (value: OpenFileResult) => void
    mockIPC(() => new Promise<OpenFileResult>(resolve => { finish = resolve }))
    const pending = getDocument("/repo/closed.ts")
    dropDocument("/repo/closed.ts")
    mockIPC(() => ({ kind: "full", content: "new", size: 3, lineEnding: "lf" }))
    await getDocument("/repo/closed.ts")
    finish({ kind: "full", content: "old", size: 3, lineEnding: "lf" })
    await expect(pending).rejects.toThrow()
    expect((await getDocument("/repo/closed.ts")).result).toMatchObject({ content: "new" })
})

test("a reload started without a cached document cannot repopulate a closed file", async () => {
    let finish!: (value: OpenFileResult) => void
    mockIPC(() => new Promise<OpenFileResult>(resolve => { finish = resolve }))
    const pending = reloadDocument("/repo/closed-reload.ts")
    dropDocument("/repo/closed-reload.ts")
    finish({ kind: "full", content: "old", size: 3, lineEnding: "lf" })
    await expect(pending).rejects.toThrow()
})

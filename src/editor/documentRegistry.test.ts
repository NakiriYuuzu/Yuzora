import { expect, test, afterEach } from "vitest"
import { mockIPC, clearMocks } from "@tauri-apps/api/mocks"
import {
    getDocument,
    updateBuffer,
    dropDocument,
    renameDocument,
    reloadDocument,
    documentGeneration,
    clearAll
} from "./documentRegistry"
import { useWorkspaceStore } from "../state/workspaceStore"
import type { OpenFileResult } from "../lib/types"
import { registerRuntimeWorkspace, saveRemoteFile } from "../lib/remoteFiles"

afterEach(() => { clearMocks(); clearAll(); useWorkspaceStore.setState({ workspacePath: null }) })

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

    expect(documentGeneration("/w/gen-old.ts")).toBe(0)
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

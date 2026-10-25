import { afterEach, beforeEach, expect, it, vi } from "vitest"
import { clearMocks, mockIPC } from "@tauri-apps/api/mocks"

import type { FileNode } from "../lib/types"
import { registerSftpWorkspace, releaseRemoteWorkspace, startRemoteWatch } from "../lib/remoteFiles"
import { useFileTreeStore } from "./fileTreeStore"
import { useSshStore } from "./sshStore"

const node = (path: string, isDir = false): FileNode => ({ path, name: path.split("/").at(-1)!, isDir })
const store = () => useFileTreeStore.getState()
let fs: Record<string, FileNode[]>
let calls: string[]

function deferred<T>() {
    let resolve!: (value: T) => void
    let reject!: (error: Error) => void
    const promise = new Promise<T>((ok, no) => { resolve = ok; reject = no })
    return { promise, resolve, reject }
}

beforeEach(() => {
    fs = { "/a": [node("/a/docs", true)], "/a/docs": [node("/a/docs/old.txt")] }
    calls = []
    useFileTreeStore.setState({ trees: {}, preciseRevision: null })
    mockIPC((command, args) => {
        if (command !== "list_dir") return null
        const path = (args as { path: string }).path
        calls.push(path)
        if (!fs[path]) throw new Error("missing fixture directory")
        return fs[path]
    })
})

afterEach(() => clearMocks())

it("full revalidation makes collapsed content fresh on next expansion without eagerly listing it", async () => {
    await store().ensureTree("/a")
    await store().toggleDir("/a", "/a/docs")
    await store().toggleDir("/a", "/a/docs")
    fs["/a/docs"] = [node("/a/docs/new.txt")]
    calls = []
    await store().ensureTree("/a")
    expect(calls).toEqual(["/a"])
    await store().toggleDir("/a", "/a/docs")
    expect(store().trees["/a"].childrenByDir["/a/docs"]).toEqual(fs["/a/docs"])
    expect(calls).toEqual(["/a", "/a/docs"])
})

it("ordinary collapse and reopen preserve the cache and issue no extra listing", async () => {
    await store().ensureTree("/a")
    await store().toggleDir("/a", "/a/docs")
    const cached = store().trees["/a"].childrenByDir["/a/docs"]
    await store().toggleDir("/a", "/a/docs")
    await store().toggleDir("/a", "/a/docs")
    expect(store().trees["/a"].childrenByDir["/a/docs"]).toBe(cached)
    expect(calls).toEqual(["/a", "/a/docs"])
})

it("workspace return invalidates only its collapsed cache and preserves expanded descendants and scroll", async () => {
    fs["/a/docs"] = [node("/a/docs/deep", true)]
    fs["/a/docs/deep"] = [node("/a/docs/deep/old.txt")]
    fs["/b"] = [node("/b/docs", true)]
    fs["/b/docs"] = []
    await store().ensureTree("/a")
    await store().toggleDir("/a", "/a/docs")
    await store().toggleDir("/a", "/a/docs/deep")
    await store().toggleDir("/a", "/a/docs")
    store().setScrollTop("/a", 123)
    await store().ensureTree("/b")
    await store().toggleDir("/b", "/b/docs")
    await store().toggleDir("/b", "/b/docs")
    const other = store().trees["/b"]
    fs["/a/docs"] = [node("/a/docs/deep", true), node("/a/docs/new.txt")]
    fs["/a/docs/deep"] = [node("/a/docs/deep/new.txt")]
    calls = []
    await store().ensureTree("/a")
    expect(calls.sort()).toEqual(["/a", "/a/docs/deep"])
    expect(store().trees["/a"].expandedDirs.has("/a/docs/deep")).toBe(true)
    expect(store().trees["/a"].scrollTop).toBe(123)
    expect(store().trees["/b"]).toBe(other)
    await store().toggleDir("/a", "/a/docs")
    expect(store().trees["/a"].childrenByDir["/a/docs"]).toEqual(fs["/a/docs"])
    expect(store().trees["/a"].childrenByDir["/a/docs/deep"]).toEqual(fs["/a/docs/deep"])
})

it.each([false, true])("refresh invalidates a collapsed in-flight listing (cached: %s)", async cached => {
    await store().ensureTree("/a")
    if (cached) await store().toggleDir("/a", "/a/docs")
    const old = deferred<FileNode[]>()
    let docsCalls = 0
    mockIPC((command, args) => {
        if (command !== "list_dir") return null
        const path = (args as { path: string }).path
        if (path === "/a/docs" && ++docsCalls === 1) return old.promise
        return fs[path]
    })
    const pending = cached
        ? store().invalidatePaths("/a", ["/a/docs/changed.txt"])
        : store().toggleDir("/a", "/a/docs")
    await vi.waitFor(() => expect(docsCalls).toBe(1))
    await store().toggleDir("/a", "/a/docs")
    await store().ensureTree("/a")
    old.resolve([node("/a/docs/obsolete.txt")])
    await pending
    fs["/a/docs"] = [node("/a/docs/current.txt")]
    await store().toggleDir("/a", "/a/docs")
    expect(store().trees["/a"].childrenByDir["/a/docs"]).toEqual(fs["/a/docs"])
    expect(docsCalls).toBe(2)
})

it.each(["success", "failure"])("old %s cannot affect expansion while the refresh root is pending", async outcome => {
    await store().ensureTree("/a")
    const old = deferred<FileNode[]>(), root = deferred<FileNode[]>()
    let docsCalls = 0
    mockIPC((command, args) => {
        if (command !== "list_dir") return null
        const path = (args as { path: string }).path
        if (path === "/a") return root.promise
        return ++docsCalls === 1 ? old.promise : [node("/a/docs/current.txt")]
    })
    const pending = store().toggleDir("/a", "/a/docs")
    await store().toggleDir("/a", "/a/docs")
    const refresh = store().ensureTree("/a")
    await store().toggleDir("/a", "/a/docs")
    if (outcome === "success") old.resolve([node("/a/docs/obsolete.txt")])
    else old.reject(new Error("obsolete fixture failure"))
    await pending
    root.resolve(fs["/a"])
    await refresh
    expect(store().trees["/a"].childrenByDir["/a/docs"]).toEqual([node("/a/docs/current.txt")])
    expect(store().trees["/a"].expandedDirs.has("/a/docs")).toBe(true)
})

it.each(["none", "success", "failure", "uncached-success", "uncached-failure"])("invalidates a directory collapsed while queued, including an old %s reply", async outcome => {
    const dirs = Array.from({ length: 10 }, (_, i) => `/a/dir${i}`)
    const target = dirs.at(-1)!, deep = `${target}/deep`
    fs["/a"] = dirs.map(path => node(path, true))
    for (const dir of [...dirs, deep]) fs[dir] = [node(`${dir}/current.txt`)]
    fs[target] = [node(deep, true), node(`${target}/current.txt`)]
    const childrenByDir = { ...Object.fromEntries([...dirs, deep].map(dir => [dir, [node(`${dir}/old.txt`)]])), [target]: [node(deep, true), node(`${target}/old.txt`)] }
    if (outcome.startsWith("uncached")) delete childrenByDir[target]
    useFileTreeStore.setState({ trees: { "/a": {
        rootNodes: fs["/a"],
        childrenByDir,
        expandedDirs: new Set([...dirs, deep].filter(dir => !outcome.startsWith("uncached") || dir !== target)), scrollTop: 0,
    } } })
    const gate = deferred<void>(), old = deferred<FileNode[]>()
    let blocked = 0, targetCalls = 0
    mockIPC((command, args) => {
        if (command !== "list_dir") return null
        const path = (args as { path: string }).path
        if (path === target) {
            targetCalls++
            return outcome !== "none" && targetCalls === 1 ? old.promise : fs[path]
        }
        blocked++
        return gate.promise.then(() => fs[path])
    })
    const pending = outcome === "none" ? Promise.resolve()
        : outcome.startsWith("uncached") ? store().toggleDir("/a", target)
            : store().invalidatePaths("/a", [`${target}/changed.txt`])
    if (outcome !== "none") await vi.waitFor(() => expect(targetCalls).toBe(1))
    const refresh = store().ensureTree("/a")
    await vi.waitFor(() => expect(blocked).toBe(8))
    await store().toggleDir("/a", target)
    gate.resolve()
    await refresh
    expect(targetCalls).toBe(outcome === "none" ? 0 : 1)
    if (outcome.endsWith("success")) old.resolve([node(`${target}/obsolete.txt`)])
    if (outcome.endsWith("failure")) old.reject(new Error("obsolete queued fixture failure"))
    await pending
    expect(store().trees["/a"].expandedDirs.has(deep)).toBe(true)
    await store().toggleDir("/a", target)
    expect(store().trees["/a"].childrenByDir[target]).toEqual(fs[target])
    expect(store().trees["/a"].childrenByDir[deep]).toEqual(fs[deep])
    expect(targetCalls).toBe(outcome === "none" ? 1 : 2)
})

it("refreshes a collapsed SFTP directory through the real remote routing without watcher events", async () => {
    const previousSessions = useSshStore.getState().sessions
    const hostId = "tree108-owned", sessionId = "tree108-fixture"
    useSshStore.setState({ sessions: { [hostId]: { hostId, sessionId, status: "connected", fingerprint: null, knownHost: false, error: null } } })
    let file = "old.txt"
    const commands: string[] = []
    mockIPC((command, args) => {
        commands.push(command)
        if (command !== "sftp_list_dir") return null
        expect((args as { sessionId: string }).sessionId).toBe(sessionId)
        const path = (args as { path: string }).path
        calls.push(path)
        return { cwd: path, entries: path === "/a"
            ? [{ name: "docs", path: "/a/docs", isDir: true, isSymlink: false, nameSafe: true }]
            : [{ name: file, path: `/a/docs/${file}`, isDir: false, isSymlink: false, nameSafe: true }] }
    })
    let uri: string | undefined
    try {
        uri = await registerSftpWorkspace(hostId, "/a")
        await startRemoteWatch(uri)
        await store().ensureTree(uri)
        const docs = store().trees[uri].rootNodes[0].path
        await store().toggleDir(uri, docs)
        await store().toggleDir(uri, docs)
        file = "new.txt"
        calls = []
        await store().ensureTree(uri)
        await store().toggleDir(uri, docs)
        expect(store().trees[uri].childrenByDir[docs].map(entry => entry.name)).toEqual(["new.txt"])
        expect(calls).toEqual(["/a", "/a/docs"])
        expect(commands).not.toContain("host_stream_open")
    } finally {
        if (uri) await releaseRemoteWorkspace(uri)
        useSshStore.setState({ sessions: previousSessions })
    }
})

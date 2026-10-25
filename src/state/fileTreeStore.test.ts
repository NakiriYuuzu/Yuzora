import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"
import { clearMocks, mockIPC } from "@tauri-apps/api/mocks"

import type { FileNode } from "../lib/types"
import { REVALIDATE_CONCURRENCY, useFileTreeStore } from "./fileTreeStore"
import { useWorkspaceStore } from "./workspaceStore"

function entry(path: string, isDir: boolean): FileNode {
    return { name: path.slice(path.lastIndexOf("/") + 1), path, isDir }
}

// Mutable fake filesystem: dir path → listing. list_dir on a missing key
// rejects (the directory is gone), mirroring the Rust command's error path.
let fakeFs: Record<string, FileNode[]>
let listCalls: string[]

function mockFs() {
    mockIPC((cmd, args) => {
        if (cmd === "list_dir") {
            const path = (args as { path: string }).path
            listCalls.push(path)
            const nodes = fakeFs[path]
            if (!nodes) return Promise.reject(new Error(`missing dir: ${path}`))
            return nodes
        }
        if (cmd === "log_event") return null
    })
}

beforeEach(() => {
    fakeFs = {}
    listCalls = []
    mockFs()
    useFileTreeStore.setState({ trees: {}, preciseRevision: null })
    useWorkspaceStore.setState({ treeRevision: 0 })
})

afterEach(() => {
    clearMocks()
})

describe("fileTreeStore — hydrate 與展開狀態", () => {
    it("ensureTree 冷載入：list root 並存進 rootNodes", async () => {
        fakeFs["/a"] = [entry("/a/src", true), entry("/a/readme.md", false)]
        await useFileTreeStore.getState().ensureTree("/a")
        expect(useFileTreeStore.getState().trees["/a"]?.rootNodes).toEqual(fakeFs["/a"])
        expect(listCalls).toEqual(["/a"])
    })

    it("toggleDir 首次展開 list 一次；收合再展開走快取不重 list", async () => {
        fakeFs["/a"] = [entry("/a/src", true)]
        fakeFs["/a/src"] = [entry("/a/src/main.ts", false)]
        await useFileTreeStore.getState().ensureTree("/a")
        await useFileTreeStore.getState().toggleDir("/a", "/a/src")
        expect(useFileTreeStore.getState().trees["/a"]?.expandedDirs.has("/a/src")).toBe(true)
        expect(useFileTreeStore.getState().trees["/a"]?.childrenByDir["/a/src"]).toEqual(
            fakeFs["/a/src"]
        )
        await useFileTreeStore.getState().toggleDir("/a", "/a/src")
        expect(useFileTreeStore.getState().trees["/a"]?.expandedDirs.has("/a/src")).toBe(false)
        await useFileTreeStore.getState().toggleDir("/a", "/a/src")
        expect(listCalls.filter((p) => p === "/a/src")).toHaveLength(1)
    })

    it("A→B→A：A 的樹狀態保留；ensureTree 背景 revalidate root＋展開目錄並 diff-apply", async () => {
        fakeFs["/a"] = [entry("/a/src", true)]
        fakeFs["/a/src"] = [entry("/a/src/main.ts", false)]
        fakeFs["/b"] = [entry("/b/only.md", false)]
        await useFileTreeStore.getState().ensureTree("/a")
        await useFileTreeStore.getState().toggleDir("/a", "/a/src")

        await useFileTreeStore.getState().ensureTree("/b")
        // 切到 B 不影響 A 的分桶。
        expect(useFileTreeStore.getState().trees["/a"]?.expandedDirs.has("/a/src")).toBe(true)

        // 外部世界變了：A 的 src 多了一個檔案。
        fakeFs["/a/src"] = [entry("/a/src/main.ts", false), entry("/a/src/new.ts", false)]
        listCalls = []
        await useFileTreeStore.getState().ensureTree("/a")
        expect(listCalls.sort()).toEqual(["/a", "/a/src"])
        expect(useFileTreeStore.getState().trees["/a"]?.childrenByDir["/a/src"]).toEqual(
            fakeFs["/a/src"]
        )
        expect(useFileTreeStore.getState().trees["/a"]?.expandedDirs.has("/a/src")).toBe(true)
    })

    it("revalidate 內容未變時保留原 children 參照（diff-apply 不換 reference）", async () => {
        fakeFs["/a"] = [entry("/a/src", true)]
        fakeFs["/a/src"] = [entry("/a/src/main.ts", false)]
        await useFileTreeStore.getState().ensureTree("/a")
        await useFileTreeStore.getState().toggleDir("/a", "/a/src")
        const before = useFileTreeStore.getState().trees["/a"]
        await useFileTreeStore.getState().ensureTree("/a")
        const after = useFileTreeStore.getState().trees["/a"]
        expect(after?.rootNodes).toBe(before?.rootNodes)
        expect(after?.childrenByDir["/a/src"]).toBe(before?.childrenByDir["/a/src"])
    })

    it("revalidate 併發上限為 REVALIDATE_CONCURRENCY", async () => {
        const dirs = Array.from({ length: 20 }, (_, i) => `/a/d${i}`)
        fakeFs["/a"] = dirs.map((d) => entry(d, true))
        let active = 0
        let maxActive = 0
        mockIPC((cmd, args) => {
            if (cmd !== "list_dir") return null
            const path = (args as { path: string }).path
            active += 1
            maxActive = Math.max(maxActive, active)
            return new Promise((resolve) => {
                setTimeout(() => {
                    active -= 1
                    resolve(fakeFs[path] ?? [])
                }, 0)
            })
        })
        useFileTreeStore.setState({
            trees: {
                "/a": {
                    rootNodes: fakeFs["/a"],
                    childrenByDir: Object.fromEntries(dirs.map((d) => [d, []])),
                    expandedDirs: new Set(dirs),
                    scrollTop: 0
                }
            }
        })
        await useFileTreeStore.getState().ensureTree("/a")
        expect(maxActive).toBe(REVALIDATE_CONCURRENCY)
    })

    it("setScrollTop 分桶保存；未知 root no-op", async () => {
        fakeFs["/a"] = []
        await useFileTreeStore.getState().ensureTree("/a")
        useFileTreeStore.getState().setScrollTop("/a", 120)
        expect(useFileTreeStore.getState().trees["/a"]?.scrollTop).toBe(120)
        useFileTreeStore.getState().setScrollTop("/zzz", 50)
        expect(useFileTreeStore.getState().trees["/zzz"]).toBeUndefined()
    })
})

describe("fileTreeStore — 精準失效 invalidatePaths", () => {
    it("initial root reconciliation refreshes cached descendants without opening new subtrees", async () => {
        fakeFs["/a"] = [entry("/a/open", true), entry("/a/closed", true)]
        fakeFs["/a/open"] = [entry("/a/open/old.txt", false)]
        fakeFs["/a/closed"] = [entry("/a/closed/unread.txt", false)]
        const store = useFileTreeStore.getState()
        await store.ensureTree("/a")
        await store.toggleDir("/a", "/a/open")
        fakeFs["/a"] = [...fakeFs["/a"], entry("/a/new.txt", false)]
        fakeFs["/a/open"] = [entry("/a/open/new.txt", false)]
        listCalls = []
        await store.invalidatePaths("/a", ["/a"])
        const tree = useFileTreeStore.getState().trees["/a"]
        expect(listCalls.sort()).toEqual(["/a", "/a/open"])
        expect(tree.rootNodes).toEqual(fakeFs["/a"])
        expect(tree.childrenByDir["/a/open"]).toEqual(fakeFs["/a/open"])
        expect(tree.childrenByDir["/a/closed"]).toBeUndefined()
        expect([...tree.expandedDirs]).toEqual(["/a/open"])
    })
    it.each(["/fixture", "C:\\fixture"])("prunes multiple removed subtrees without touching prefix siblings or prior snapshots under %s", async root => {
        const separator = root.startsWith("C:") ? "\\" : "/"
        const path = (suffix: string) => root + separator + suffix.replaceAll("/", separator)
        const cached = ["gone", "gone/deep", "other", "other/deep", "gone-keep", "gone-keep/deep", "keep"]
        const before = {
            rootNodes: ["gone", "other", "gone-keep", "keep"].map(dir => entry(path(dir), true)),
            childrenByDir: Object.fromEntries(cached.map(dir => [path(dir), []])),
            expandedDirs: new Set(cached.map(path)),
            scrollTop: 123
        }
        useFileTreeStore.setState({ trees: { [root]: before } })
        fakeFs[root] = [entry(path("gone-keep"), true), entry(path("keep"), true)]
        await useFileTreeStore.getState().invalidatePaths(root, [path("changed.txt")])
        const after = useFileTreeStore.getState().trees[root]
        expect(Object.keys(after.childrenByDir).sort()).toEqual(["gone-keep", "gone-keep/deep", "keep"].map(path).sort())
        expect([...after.expandedDirs].sort()).toEqual(["gone-keep", "gone-keep/deep", "keep"].map(path).sort())
        expect(after.scrollTop).toBe(123)
        expect(Object.keys(before.childrenByDir)).toHaveLength(cached.length)
        expect(before.expandedDirs.size).toBe(cached.length)
        expect(listCalls).toEqual([root])
    })

    it("preserves child-cache and expansion identity when a root file alone changes", async () => {
        const before = { rootNodes: [entry("/a/src", true)], childrenByDir: { "/a/src": [] }, expandedDirs: new Set(["/a/src"]), scrollTop: 0 }
        useFileTreeStore.setState({ trees: { "/a": before } })
        fakeFs["/a"] = [...before.rootNodes, entry("/a/new.ts", false)]
        await useFileTreeStore.getState().invalidatePaths("/a", ["/a/new.ts"])
        const after = useFileTreeStore.getState().trees["/a"]
        expect(after.rootNodes).toEqual(fakeFs["/a"])
        expect(after.childrenByDir).toBe(before.childrenByDir)
        expect(after.expandedDirs).toBe(before.expandedDirs)
    })

    it("只 re-list 受影響且已快取的目錄；展開狀態保留；bump treeRevision 並留 marker", async () => {
        fakeFs["/a"] = [entry("/a/src", true)]
        fakeFs["/a/src"] = [entry("/a/src/main.ts", false)]
        await useFileTreeStore.getState().ensureTree("/a")
        await useFileTreeStore.getState().toggleDir("/a", "/a/src")

        fakeFs["/a/src"] = [entry("/a/src/main.ts", false), entry("/a/src/new.ts", false)]
        listCalls = []
        await useFileTreeStore.getState().invalidatePaths("/a", ["/a/src/new.ts"])
        expect(listCalls).toEqual(["/a/src"])
        expect(useFileTreeStore.getState().trees["/a"]?.childrenByDir["/a/src"]).toEqual(
            fakeFs["/a/src"]
        )
        expect(useFileTreeStore.getState().trees["/a"]?.expandedDirs.has("/a/src")).toBe(true)
        expect(useWorkspaceStore.getState().treeRevision).toBe(1)
        // marker 一次性：同 revision 消費一次為 true，再問為 false。
        expect(useFileTreeStore.getState().consumePreciseRevision("/a", 1)).toBe(true)
        expect(useFileTreeStore.getState().consumePreciseRevision("/a", 1)).toBe(false)
    })

    it("coalesced directory relists only cached descendants once, not targetX or uncached subtrees", async () => {
        fakeFs["/a"] = [entry("/a/target", true), entry("/a/targetX", true)]
        fakeFs["/a/target"] = [entry("/a/target/debug", true), entry("/a/target/uncached", true)]
        fakeFs["/a/target/debug"] = [entry("/a/target/debug/out", false)]
        fakeFs["/a/targetX"] = [entry("/a/targetX/other", false)]
        const store = useFileTreeStore.getState()
        await store.ensureTree("/a")
        for (const dir of ["/a/target", "/a/target/debug", "/a/targetX"]) await store.toggleDir("/a", dir)
        fakeFs["/a/target/debug"] = [entry("/a/target/debug/new", false)]
        listCalls = []

        await store.invalidatePaths("/a", ["/a/target", "/a/target", "/a/target/debug/out"])

        expect(listCalls.sort()).toEqual(["/a", "/a/target", "/a/target/debug"])
        const tree = useFileTreeStore.getState().trees["/a"]
        expect(tree?.childrenByDir["/a/target/debug"]).toEqual(fakeFs["/a/target/debug"])
        expect(tree?.childrenByDir["/a/target/uncached"]).toBeUndefined()
        expect(tree?.expandedDirs.has("/a/target/debug")).toBe(true)
    })

    it("prunes a removed nested directory before listing coalesced descendants", async () => {
        fakeFs["/a"] = [entry("/a/src", true)]
        fakeFs["/a/src"] = [entry("/a/src/nested", true), entry("/a/src/keep", true)]
        fakeFs["/a/src/nested"] = [entry("/a/src/nested/deep", true)]
        fakeFs["/a/src/nested/deep"] = []
        fakeFs["/a/src/keep"] = []
        const store = useFileTreeStore.getState()
        await store.ensureTree("/a")
        for (const dir of ["/a/src", "/a/src/nested", "/a/src/nested/deep", "/a/src/keep"])
            await store.toggleDir("/a", dir)
        // Keep stale fake child listings: requesting them would resurrect the cache.
        fakeFs["/a/src"] = [entry("/a/src/keep", true)]
        listCalls = []

        await store.invalidatePaths("/a", ["/a/src", "/a/src/nested/deep/file"])

        expect(listCalls).toEqual(["/a", "/a/src", "/a/src/keep"])
        const tree = useFileTreeStore.getState().trees["/a"]!
        expect(Object.keys(tree.childrenByDir)).toEqual(["/a/src", "/a/src/keep"])
        expect([...tree.expandedDirs]).toEqual(["/a/src", "/a/src/keep"])
    })

    it("coalesced Windows directory matches cached casing and separators without matching targetX", async () => {
        const root = String.raw`C:\Work`
        const target = String.raw`C:\Work\Target`
        const nested = String.raw`C:\Work\Target\Debug`
        const other = String.raw`C:\Work\TargetX`
        fakeFs[root] = [entry(target, true), entry(other, true)]
        fakeFs[target] = [entry(nested, true)]
        fakeFs[nested] = []
        fakeFs[other] = []
        const store = useFileTreeStore.getState()
        await store.ensureTree(root)
        for (const dir of [target, nested, other]) await store.toggleDir(root, dir)
        listCalls = []

        await store.invalidatePaths(root, ["c:/work/target"])

        expect(listCalls.sort()).toEqual([target, nested].sort())
    })

    it("root 直屬路徑變更 → re-list root", async () => {
        fakeFs["/a"] = [entry("/a/readme.md", false)]
        await useFileTreeStore.getState().ensureTree("/a")
        fakeFs["/a"] = [entry("/a/readme.md", false), entry("/a/new.ts", false)]
        listCalls = []
        await useFileTreeStore.getState().invalidatePaths("/a", ["/a/new.ts"])
        expect(listCalls).toEqual(["/a"])
        expect(useFileTreeStore.getState().trees["/a"]?.rootNodes).toEqual(fakeFs["/a"])
    })

    it("父目錄未快取（未展開過）→ 不 re-list，仍 bump revision", async () => {
        fakeFs["/a"] = [entry("/a/deep", true)]
        await useFileTreeStore.getState().ensureTree("/a")
        listCalls = []
        await useFileTreeStore.getState().invalidatePaths("/a", ["/a/deep/x.ts"])
        expect(listCalls).toEqual([])
        expect(useWorkspaceStore.getState().treeRevision).toBe(1)
    })

    it("tree 尚未載入的 root → 只 bump revision、無 marker（FileTree 會 fallback 全載）", async () => {
        await useFileTreeStore.getState().invalidatePaths("/never", ["/never/x.ts"])
        expect(useWorkspaceStore.getState().treeRevision).toBe(1)
        expect(useFileTreeStore.getState().consumePreciseRevision("/never", 1)).toBe(false)
    })

    it("目錄被刪除：re-list parent 後 prune 該子樹的 children 快取與展開旗標", async () => {
        fakeFs["/a"] = [entry("/a/src", true)]
        fakeFs["/a/src"] = [entry("/a/src/nested", true)]
        fakeFs["/a/src/nested"] = [entry("/a/src/nested/x.ts", false)]
        await useFileTreeStore.getState().ensureTree("/a")
        await useFileTreeStore.getState().toggleDir("/a", "/a/src")
        await useFileTreeStore.getState().toggleDir("/a", "/a/src/nested")

        fakeFs["/a"] = []
        delete fakeFs["/a/src"]
        delete fakeFs["/a/src/nested"]
        listCalls = []
        await useFileTreeStore.getState().invalidatePaths("/a", ["/a/src"])
        const tree = useFileTreeStore.getState().trees["/a"]
        expect(listCalls).toEqual(["/a"])
        expect(tree?.rootNodes).toEqual([])
        expect(tree?.childrenByDir["/a/src"]).toBeUndefined()
        expect(tree?.childrenByDir["/a/src/nested"]).toBeUndefined()
        expect(tree?.expandedDirs.has("/a/src")).toBe(false)
        expect(tree?.expandedDirs.has("/a/src/nested")).toBe(false)
    })

    it("revalidate 中某展開目錄 list 失敗（已刪）→ 丟棄該子樹快取，其餘照常", async () => {
        fakeFs["/a"] = [entry("/a/src", true), entry("/a/lib", true)]
        fakeFs["/a/src"] = [entry("/a/src/main.ts", false)]
        fakeFs["/a/lib"] = [entry("/a/lib/util.ts", false)]
        await useFileTreeStore.getState().ensureTree("/a")
        await useFileTreeStore.getState().toggleDir("/a", "/a/src")
        await useFileTreeStore.getState().toggleDir("/a", "/a/lib")

        delete fakeFs["/a/src"]
        await useFileTreeStore.getState().ensureTree("/a")
        const tree = useFileTreeStore.getState().trees["/a"]
        expect(tree?.childrenByDir["/a/src"]).toBeUndefined()
        expect(tree?.expandedDirs.has("/a/src")).toBe(false)
        expect(tree?.childrenByDir["/a/lib"]).toEqual(fakeFs["/a/lib"])
        expect(tree?.expandedDirs.has("/a/lib")).toBe(true)
    })

    it("consumePreciseRevision 只吻合同 root＋同 revision", async () => {
        fakeFs["/a"] = []
        await useFileTreeStore.getState().ensureTree("/a")
        await useFileTreeStore.getState().invalidatePaths("/a", ["/a/x.ts"])
        const revision = useWorkspaceStore.getState().treeRevision
        expect(useFileTreeStore.getState().consumePreciseRevision("/b", revision)).toBe(false)
        expect(useFileTreeStore.getState().consumePreciseRevision("/a", revision + 1)).toBe(false)
        expect(useFileTreeStore.getState().consumePreciseRevision("/a", revision)).toBe(true)
    })
})

describe("fileTreeStore — overlapping listings", () => {
    function deferred<T>() {
        let resolve!: (value: T) => void
        let reject!: (reason: Error) => void
        const promise = new Promise<T>((ok, no) => { resolve = ok; reject = no })
        return { promise, resolve, reject }
    }

    it.each(["/a", "/a/src"])("older success cannot overwrite a newer listing for %s", async dir => {
        fakeFs["/a"] = [entry("/a/src", true)]
        fakeFs["/a/src"] = []
        const store = useFileTreeStore.getState()
        await store.ensureTree("/a")
        await store.toggleDir("/a", "/a/src")
        const old = deferred<FileNode[]>()
        const current = [entry(`${dir}/current.ts`, false)]
        let calls = 0
        mockIPC((command, args) => {
            if (command === "list_dir" && (args as { path: string }).path === dir)
                return ++calls === 1 ? old.promise : current
        })
        const previous = store.invalidatePaths("/a", [`${dir}/changed.ts`])
        await store.invalidatePaths("/a", [`${dir}/changed.ts`])
        old.resolve([entry(`${dir}/obsolete.ts`, false)])
        await previous
        const tree = useFileTreeStore.getState().trees["/a"]
        expect(dir === "/a" ? tree.rootNodes : tree.childrenByDir[dir]).toEqual(current)
    })

    it("an older failure cannot prune a newer successful child listing", async () => {
        fakeFs["/a"] = [entry("/a/src", true)]
        fakeFs["/a/src"] = []
        const store = useFileTreeStore.getState()
        await store.ensureTree("/a")
        await store.toggleDir("/a", "/a/src")
        const old = deferred<FileNode[]>()
        const current = [entry("/a/src/current.ts", false)]
        let calls = 0
        mockIPC(command => command === "list_dir" ? (++calls === 1 ? old.promise : current) : undefined)
        const previous = store.invalidatePaths("/a", ["/a/src/changed.ts"])
        await store.invalidatePaths("/a", ["/a/src/changed.ts"])
        old.reject(new Error("owned old listing failed"))
        await previous
        const tree = useFileTreeStore.getState().trees["/a"]
        expect(tree.childrenByDir["/a/src"]).toEqual(current)
        expect(tree.expandedDirs.has("/a/src")).toBe(true)
    })

    it.each([false, true])("parent removal invalidates a pending child listing (already cached: %s)", async cached => {
        fakeFs["/a"] = [entry("/a/src", true), entry("/a/keep", true)]
        fakeFs["/a/src"] = []
        fakeFs["/a/keep"] = []
        const store = useFileTreeStore.getState()
        await store.ensureTree("/a")
        await store.toggleDir("/a", "/a/keep")
        if (cached) await store.toggleDir("/a", "/a/src")
        const old = deferred<FileNode[]>()
        mockIPC((command, args) => {
            if (command !== "list_dir") return
            const path = (args as { path: string }).path
            return path === "/a/src" ? old.promise : [entry("/a/keep", true)]
        })
        const previous = cached ? store.invalidatePaths("/a", ["/a/src/file"]) : store.toggleDir("/a", "/a/src")
        await store.invalidatePaths("/a", ["/a/src"])
        old.resolve([entry("/a/src/obsolete.ts", false)])
        await previous
        const tree = useFileTreeStore.getState().trees["/a"]
        expect(tree.rootNodes).toEqual([entry("/a/keep", true)])
        expect(tree.childrenByDir["/a/src"]).toBeUndefined()
        expect(tree.expandedDirs.has("/a/src")).toBe(false)
        expect(tree.childrenByDir["/a/keep"]).toEqual([])
        expect(tree.expandedDirs.has("/a/keep")).toBe(true)
    })

    it("keeps pending sibling and other-workspace listings independent", async () => {
        fakeFs["/a"] = [entry("/a/src", true), entry("/a/keep", true)]
        fakeFs["/a/src"] = []
        fakeFs["/a/keep"] = []
        fakeFs["/b"] = []
        const store = useFileTreeStore.getState()
        await store.ensureTree("/a")
        await store.ensureTree("/b")
        for (const dir of ["/a/src", "/a/keep"]) await store.toggleDir("/a", dir)
        const old = deferred<FileNode[]>()
        const sibling = deferred<FileNode[]>()
        const other = deferred<FileNode[]>()
        let srcCalls = 0
        mockIPC((command, args) => {
            if (command !== "list_dir") return
            const path = (args as { path: string }).path
            if (path === "/a/src") return ++srcCalls === 1 ? old.promise : [entry("/a/src/current", false)]
            if (path === "/a/keep") return sibling.promise
            if (path === "/b") return other.promise
        })
        const pending = [
            store.invalidatePaths("/a", ["/a/src/change"]),
            store.invalidatePaths("/a", ["/a/keep/change"]),
            store.invalidatePaths("/b", ["/b/change"]),
        ]
        await store.invalidatePaths("/a", ["/a/src/change"])
        old.reject(new Error("owned obsolete failure"))
        sibling.resolve([entry("/a/keep/current", false)])
        other.resolve([entry("/b/current", false)])
        await Promise.all(pending)
        const trees = useFileTreeStore.getState().trees
        expect(trees["/a"].childrenByDir["/a/src"]).toEqual([entry("/a/src/current", false)])
        expect(trees["/a"].childrenByDir["/a/keep"]).toEqual([entry("/a/keep/current", false)])
        expect(trees["/b"].rootNodes).toEqual([entry("/b/current", false)])
    })

    it("a current parent failure invalidates a pending descendant without clearing the root listing", async () => {
        fakeFs["/a"] = [entry("/a/src", true)]
        fakeFs["/a/src"] = [entry("/a/src/deep", true)]
        fakeFs["/a/src/deep"] = []
        const store = useFileTreeStore.getState()
        await store.ensureTree("/a")
        for (const dir of ["/a/src", "/a/src/deep"]) await store.toggleDir("/a", dir)
        const descendant = deferred<FileNode[]>()
        mockIPC((command, args) => {
            if (command !== "list_dir") return
            if ((args as { path: string }).path === "/a/src/deep") return descendant.promise
            throw new Error("owned current parent failure")
        })
        const pending = store.invalidatePaths("/a", ["/a/src/deep/change"])
        await store.invalidatePaths("/a", ["/a/src/change"])
        descendant.resolve([entry("/a/src/deep/obsolete", false)])
        await pending
        const tree = useFileTreeStore.getState().trees["/a"]
        expect(tree.rootNodes).toEqual(fakeFs["/a"])
        expect(tree.childrenByDir).toEqual({})
        expect(tree.expandedDirs.size).toBe(0)
    })

    it("does not start queued revalidation jobs after their parent prunes them", async () => {
        const dirs = Array.from({ length: REVALIDATE_CONCURRENCY + 4 }, (_, i) => `/a/dir${i}`)
        useFileTreeStore.setState({ trees: { "/a": {
            rootNodes: dirs.map(dir => entry(dir, true)),
            childrenByDir: Object.fromEntries(dirs.map(dir => [dir, []])),
            expandedDirs: new Set(dirs), scrollTop: 0,
        } } })
        const root = deferred<FileNode[]>()
        const children = deferred<FileNode[]>()
        const calls: string[] = []
        mockIPC((command, args) => {
            if (command !== "list_dir") return
            const path = (args as { path: string }).path
            calls.push(path)
            return path === "/a" ? root.promise : children.promise
        })
        const revalidating = useFileTreeStore.getState().ensureTree("/a")
        expect(calls).toHaveLength(REVALIDATE_CONCURRENCY)
        root.resolve([])
        await vi.waitFor(() => expect(useFileTreeStore.getState().trees["/a"].rootNodes).toEqual([]))
        children.resolve([])
        await revalidating
        expect(calls).toHaveLength(REVALIDATE_CONCURRENCY)
        expect(useFileTreeStore.getState().trees["/a"].childrenByDir).toEqual({})
        expect(useFileTreeStore.getState().trees["/a"].expandedDirs.size).toBe(0)
    })
})

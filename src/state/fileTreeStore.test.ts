import { afterEach, beforeEach, describe, expect, it } from "vitest"
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
        await useFileTreeStore.getState().invalidatePaths("/a", ["/a/src"])
        const tree = useFileTreeStore.getState().trees["/a"]
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

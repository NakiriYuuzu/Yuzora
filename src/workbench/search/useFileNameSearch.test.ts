import { act, cleanup, renderHook } from "@testing-library/react"
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"
import type { FileNode } from "@/lib/types"
import { remoteFilePath } from "@/lib/runtimeIdentity"

const listDir = vi.hoisted(() => vi.fn<(path: string) => Promise<FileNode[]>>())
vi.mock("@/lib/ipc", () => ({ listDir }))
import { useFileNameSearch } from "./useFileNameSearch"

const file = (path: string): FileNode => ({ name: path.split(/[\\/]/).at(-1)!, path, isDir: false, kind: "file" })
const dir = (path: string): FileNode => ({ ...file(path), isDir: true, kind: "directory" })
const tick = () => act(async () => { await vi.advanceTimersByTimeAsync(250) })
function deferred() {
    let resolve!: (nodes: FileNode[]) => void
    const promise = new Promise<FileNode[]>((done) => { resolve = done })
    return { promise, resolve }
}

beforeEach(() => { vi.useFakeTimers(); listDir.mockReset(); listDir.mockResolvedValue([]) })
afterEach(() => { cleanup(); vi.useRealTimers() })

describe("useFileNameSearch", () => {
    it("debounces trimmed case-insensitive queries and traverses collapsed directories including node_modules", async () => {
        listDir.mockImplementation(async (path) => ({
            "/w": [dir("/w/src"), dir("/w/node_modules"), file("/w/README.md")],
            "/w/src": [dir("/w/src/deep")],
            "/w/src/deep": [file("/w/src/deep/Target.ts")],
            "/w/node_modules": [file("/w/node_modules/target.js")],
        })[path] ?? [])
        const { result, rerender } = renderHook(({ q }) => useFileNameSearch("/w", q, 0, true), { initialProps: { q: "  TARGET  " } })
        await act(async () => { await vi.advanceTimersByTimeAsync(249) })
        expect(listDir).not.toHaveBeenCalled()
        await tick()
        expect(result.current.files.map((node) => node.path)).toEqual(["/w/node_modules/target.js", "/w/src/deep/Target.ts"])
        expect(result.current).toMatchObject({ loading: false, incomplete: false, error: null })
        rerender({ q: "SRC\\DEEP" })
        expect(result.current.files).toEqual([])
        await tick()
        expect(result.current.files.map((node) => node.path)).toEqual(["/w/src/deep/Target.ts"])
    })

    it("uses Windows containment without changing operational paths", async () => {
        const match = file("c:\\WORK\\Nested\\Target.ts")
        listDir.mockResolvedValueOnce([dir("c:\\WORK\\Nested"), file("C:\\WorkElse\\Target.ts")]).mockResolvedValueOnce([match])
        const { result } = renderHook(() => useFileNameSearch("C:\\Work", "nested/target", 0, true))
        await tick()
        expect(result.current.files).toEqual([match])
        expect(listDir.mock.calls).toEqual([["C:\\Work"], ["c:\\WORK\\Nested"]])
    })

    it("preserves remote host and workspace boundaries", async () => {
        const root = remoteFilePath("host-a", "/w")
        const nested = remoteFilePath("host-a", "/w/deep", "/w")
        const match = file(remoteFilePath("host-a", "/w/deep/Target.ts", "/w"))
        listDir.mockResolvedValueOnce([
            dir(nested), dir(remoteFilePath("host-b", "/w/deep", "/w")),
            dir(remoteFilePath("host-a", "/w/deep", "/another")),
        ]).mockResolvedValueOnce([match])
        const { result } = renderHook(() => useFileNameSearch(root, "deep/target", 0, true))
        await tick()
        expect(result.current.files).toEqual([match])
        expect(listDir.mock.calls).toEqual([[root], [nested]])
    })

    it("never traverses symlinks, metadata, outside paths, or dot segments and returns only files", async () => {
        listDir.mockResolvedValueOnce([
            { ...dir("/w/link"), kind: "symlink" }, { ...file("/w/target-link"), kind: "symlink" },
            { ...file("/w/target-device"), kind: "other" }, dir("/w/.git"),
            dir("/outside"), dir("/w/../escape"), dir("/w/./ambiguous"), dir("/w"),
            dir("/w/target"), file("/w/target.txt"),
        ]).mockResolvedValueOnce([file("/w/sibling-target.txt")])
        const { result } = renderHook(() => useFileNameSearch("/w", "target", 0, true))
        await tick()
        expect(listDir.mock.calls).toEqual([["/w"], ["/w/target"]])
        expect(result.current.files).toEqual([file("/w/target.txt")])
    })

    it("reports root errors and marks subtree failures incomplete", async () => {
        listDir.mockRejectedValueOnce(new Error("Permission denied"))
        const { result, rerender } = renderHook(({ revision }) => useFileNameSearch("/w", "target", revision, true), { initialProps: { revision: 0 } })
        await tick()
        expect(result.current).toMatchObject({ error: "Permission denied", loading: false })
        listDir.mockResolvedValueOnce([dir("/w/closed"), file("/w/target.ts")]).mockRejectedValueOnce(new Error("Denied"))
        rerender({ revision: 1 })
        await tick()
        expect(result.current).toMatchObject({ files: [file("/w/target.ts")], error: null, incomplete: true, loading: false })
    })

    it("caps matches at 200", async () => {
        listDir.mockResolvedValueOnce(Array.from({ length: 250 }, (_, i) => file(`/w/target-${i}`)))
        const { result } = renderHook(() => useFileNameSearch("/w", "target", 0, true))
        await tick()
        expect(result.current.files).toHaveLength(200)
        expect(result.current.incomplete).toBe(true)
    })

    it("caps directory traversal at 2000 including the root", async () => {
        listDir.mockResolvedValueOnce(Array.from({ length: 2050 }, (_, i) => dir(`/w/dir-${i}`)))
        const { result } = renderHook(() => useFileNameSearch("/w", "target", 0, true))
        await tick()
        expect(listDir).toHaveBeenCalledTimes(2000)
        expect(result.current).toMatchObject({ files: [], incomplete: true, loading: false })
    })

    it.each(["query", "root", "revision", "active"])("cancels pending traversal on %s changes", async (change) => {
        const old = deferred()
        listDir.mockReturnValueOnce(old.promise)
        const props = { root: "/w", q: "target", revision: 0, active: true }
        const { result, rerender } = renderHook((p) => useFileNameSearch(p.root, p.q, p.revision, p.active), { initialProps: props })
        await tick()
        rerender({ ...props, ...({ query: { q: "new" }, root: { root: "/new" }, revision: { revision: 1 }, active: { active: false } })[change] })
        await act(async () => { old.resolve([dir("/w/stale"), file("/w/target-old")]) })
        expect(result.current.files).toEqual([])
        expect(listDir).not.toHaveBeenCalledWith("/w/stale")
        await tick()
        expect(result.current.loading).toBe(false)
        expect(listDir).toHaveBeenCalledTimes(change === "active" ? 1 : 2)
    })

    it("does not expose completed results when a query or active state returns to its old value", async () => {
        listDir.mockResolvedValue([file("/w/target")])
        const { result, rerender } = renderHook(({ q, active }) => useFileNameSearch("/w", q, 0, active), { initialProps: { q: "target", active: true } })
        await tick()
        expect(result.current.files).toHaveLength(1)
        rerender({ q: "other", active: true })
        rerender({ q: "target", active: true })
        expect(result.current).toMatchObject({ files: [], loading: true })
        await tick()
        rerender({ q: "target", active: false })
        expect(result.current).toMatchObject({ files: [], loading: false })
        rerender({ q: "target", active: true })
        expect(result.current).toMatchObject({ files: [], loading: true })
    })

    it("shares four listing slots with cancelled generations", async () => {
        const old = Array.from({ length: 4 }, deferred)
        listDir.mockResolvedValueOnce(old.map((_, i) => dir(`/w/${i}`)))
        for (const pending of old) listDir.mockReturnValueOnce(pending.promise)
        const { result, rerender } = renderHook(({ revision }) => useFileNameSearch("/w", "target", revision, true), { initialProps: { revision: 0 } })
        await tick()
        expect(listDir).toHaveBeenCalledTimes(5)
        rerender({ revision: 1 })
        await tick()
        expect(listDir).toHaveBeenCalledTimes(5)
        await act(async () => { old[0].resolve([dir("/w/0/stale")]) })
        expect(listDir).toHaveBeenCalledTimes(6)
        expect(result.current.loading).toBe(false)
        await act(async () => { old.slice(1).forEach((pending) => pending.resolve([dir("/w/1/stale")])) })
        expect(listDir).toHaveBeenCalledTimes(6)
        expect(result.current.files).toEqual([])
    })

    it("does not list for empty queries, missing roots, or inactive search", async () => {
        renderHook(() => useFileNameSearch("/w", "  ", 0, true))
        renderHook(() => useFileNameSearch(null, "target", 0, true))
        renderHook(() => useFileNameSearch("/w", "target", 0, false))
        await tick()
        expect(listDir).not.toHaveBeenCalled()
    })
})

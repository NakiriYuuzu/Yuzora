import { act, cleanup, renderHook } from "@testing-library/react"
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"
import type { FileNode } from "@/lib/types"
import type { FileNameSearchResponse } from "@/lib/fileNameSearchTypes"
import { remoteFilePath } from "@/lib/runtimeIdentity"

const { listDir, searchWorkspaceFileNames, searchRemoteFileNames } = vi.hoisted(() => ({
    listDir: vi.fn<(path: string) => Promise<FileNode[]>>(),
    searchWorkspaceFileNames: vi.fn<(root: string, query: string) => Promise<FileNameSearchResponse>>(),
    searchRemoteFileNames: vi.fn<(root: string, query: string) => Promise<FileNameSearchResponse | null>>(),
}))
vi.mock("@/lib/ipc", () => ({ listDir, searchWorkspaceFileNames }))
vi.mock("@/lib/remoteFileNameSearch", () => ({ searchRemoteFileNames }))
import { useFileNameSearch } from "./useFileNameSearch"

const file = (path: string): FileNode => ({ name: path.split(/[\\/]/).at(-1)!, path, isDir: false, kind: "file" })
const dir = (path: string): FileNode => ({ ...file(path), isDir: true, kind: "directory" })
const remote = (path: string) => remoteFilePath("host-a", path, "/w")
const root = remote("/w")
const tick = () => act(async () => { await vi.advanceTimersByTimeAsync(250) })
function deferred<T>() {
    let resolve!: (value: T) => void
    const promise = new Promise<T>((done) => { resolve = done })
    return { promise, resolve }
}

beforeEach(() => {
    vi.useFakeTimers()
    listDir.mockReset().mockResolvedValue([])
    searchWorkspaceFileNames.mockReset().mockResolvedValue({ files: [], incomplete: false })
    searchRemoteFileNames.mockReset().mockResolvedValue(null)
})
afterEach(() => { cleanup(); vi.useRealTimers() })

describe("useFileNameSearch", () => {
    it("debounces and sends normalized relative-path queries through one local IPC", async () => {
        const match = file("/w/src/deep/Target.ts")
        searchWorkspaceFileNames.mockResolvedValue({ files: [match], incomplete: false })
        const { result, rerender } = renderHook(({ q }) => useFileNameSearch("/w", q, 0, true), { initialProps: { q: "  TARGET  " } })
        await act(async () => { await vi.advanceTimersByTimeAsync(249) })
        expect(searchWorkspaceFileNames).not.toHaveBeenCalled()
        await tick()
        expect(searchWorkspaceFileNames).toHaveBeenCalledExactlyOnceWith("/w", "target")
        expect(result.current).toEqual({ files: [match], loading: false, incomplete: false, error: null })
        rerender({ q: "SRC\\DEEP" })
        expect(result.current.files).toEqual([])
        await tick()
        expect(searchWorkspaceFileNames).toHaveBeenLastCalledWith("/w", "src/deep")
        expect(listDir).not.toHaveBeenCalled()
        expect(searchRemoteFileNames).not.toHaveBeenCalled()
    })

    it("preserves Windows operational roots and reports native incomplete/errors", async () => {
        searchWorkspaceFileNames.mockResolvedValueOnce({ files: [file("C:\\Work\\Target.ts")], incomplete: true })
        const { result, rerender } = renderHook(({ revision }) => useFileNameSearch("C:\\Work", "target", revision, true), { initialProps: { revision: 0 } })
        await tick()
        expect(searchWorkspaceFileNames).toHaveBeenCalledWith("C:\\Work", "target")
        expect(result.current.incomplete).toBe(true)
        searchWorkspaceFileNames.mockRejectedValueOnce(new Error("Permission denied"))
        rerender({ revision: 1 })
        await tick()
        expect(result.current).toMatchObject({ files: [], error: "Permission denied", loading: false })
        expect(listDir).not.toHaveBeenCalled()
    })

    it("uses supported remote search without directory IPC", async () => {
        const match = file(remote("/w/src/Target.ts"))
        searchRemoteFileNames.mockResolvedValue({ files: [match], incomplete: false })
        const { result } = renderHook(() => useFileNameSearch(root, "target", 0, true))
        await tick()
        expect(result.current.files).toEqual([match])
        expect(searchRemoteFileNames).toHaveBeenCalledExactlyOnceWith(root, "target")
        expect(listDir).not.toHaveBeenCalled()
        expect(searchWorkspaceFileNames).not.toHaveBeenCalled()
    })

    it("queues only the latest remote query while a helper request is in flight", async () => {
        const old = deferred<FileNameSearchResponse | null>()
        searchRemoteFileNames.mockReturnValueOnce(old.promise).mockResolvedValue({ files: [file(remote("/w/latest.ts"))], incomplete: false })
        const { result, rerender } = renderHook(({ q }) => useFileNameSearch(root, q, 0, true), { initialProps: { q: "old" } })
        await tick()
        rerender({ q: "middle" }); await tick()
        rerender({ q: "latest" }); await tick()
        expect(searchRemoteFileNames).toHaveBeenCalledTimes(1)
        await act(async () => { old.resolve({ files: [file(remote("/w/old.ts"))], incomplete: false }) })
        expect(searchRemoteFileNames.mock.calls).toEqual([[root, "old"], [root, "latest"]])
        expect(result.current.files).toEqual([file(remote("/w/latest.ts"))])
    })

    it("does not fall back when a supported helper fails", async () => {
        searchRemoteFileNames.mockRejectedValue(new Error("Disconnected"))
        const { result } = renderHook(() => useFileNameSearch(root, "target", 0, true))
        await tick()
        expect(result.current.error).toBe("Disconnected")
        expect(listDir).not.toHaveBeenCalled()
    })

    it("falls back for legacy remote helpers while preserving host/workspace boundaries", async () => {
        const nested = remote("/w/deep")
        const match = file(remote("/w/deep/Target.ts"))
        listDir.mockResolvedValueOnce([
            dir(nested), dir(remoteFilePath("host-b", "/w/deep", "/w")),
            dir(remoteFilePath("host-a", "/w/deep", "/another")),
        ]).mockResolvedValueOnce([match])
        const { result } = renderHook(() => useFileNameSearch(root, "deep/target", 0, true))
        await tick()
        expect(result.current.files).toEqual([match])
        expect(listDir.mock.calls).toEqual([[root], [nested]])
    })

    it("legacy traversal skips symlinks, metadata, outside paths and directory cycles", async () => {
        listDir.mockResolvedValueOnce([
            { ...dir(remote("/w/link")), kind: "symlink" }, { ...file(remote("/w/target-link")), kind: "symlink" },
            { ...file(remote("/w/target-device")), kind: "other" }, dir(remote("/w/.git")),
            dir(remote("/outside")), dir(root),
            dir(remote("/w/target")), file(remote("/w/target.txt")),
        ]).mockResolvedValueOnce([file(remote("/w/sibling-target.txt"))])
        const { result } = renderHook(() => useFileNameSearch(root, "target", 0, true))
        await tick()
        expect(listDir.mock.calls).toEqual([[root], [remote("/w/target")]])
        expect(result.current.files).toEqual([file(remote("/w/target.txt"))])
    })

    it("legacy traversal reports root errors and subtree incompleteness", async () => {
        listDir.mockRejectedValueOnce(new Error("Permission denied"))
        const { result, rerender } = renderHook(({ revision }) => useFileNameSearch(root, "target", revision, true), { initialProps: { revision: 0 } })
        await tick()
        expect(result.current).toMatchObject({ error: "Permission denied", loading: false })
        listDir.mockResolvedValueOnce([dir(remote("/w/closed")), file(remote("/w/target.ts"))]).mockRejectedValueOnce(new Error("Denied"))
        rerender({ revision: 1 }); await tick()
        expect(result.current).toMatchObject({ error: null, incomplete: true, loading: false })
        expect(result.current.files).toHaveLength(1)
    })

    it("legacy traversal caps matches at 200 and directories at 2000", async () => {
        listDir.mockResolvedValueOnce(Array.from({ length: 250 }, (_, i) => file(remote(`/w/target-${i}`))))
        const { result, rerender } = renderHook(({ revision }) => useFileNameSearch(root, "target", revision, true), { initialProps: { revision: 0 } })
        await tick()
        expect(result.current.files).toHaveLength(200)
        expect(result.current.incomplete).toBe(true)
        listDir.mockClear().mockResolvedValueOnce(Array.from({ length: 2050 }, (_, i) => dir(remote(`/w/dir-${i}`))))
        rerender({ revision: 1 }); await tick()
        expect(listDir).toHaveBeenCalledTimes(2000)
        expect(result.current).toMatchObject({ files: [], incomplete: true, loading: false })
    })

    it.each(["query", "root", "revision", "active"])("discards pending local results on %s changes", async (change) => {
        const old = deferred<FileNameSearchResponse>()
        searchWorkspaceFileNames.mockReturnValueOnce(old.promise)
        const props = { root: "/w", q: "target", revision: 0, active: true }
        const { result, rerender } = renderHook((p) => useFileNameSearch(p.root, p.q, p.revision, p.active), { initialProps: props })
        await tick()
        rerender({ ...props, ...({ query: { q: "new" }, root: { root: "/new" }, revision: { revision: 1 }, active: { active: false } })[change] })
        await tick()
        await act(async () => { old.resolve({ files: [file("/w/target-old")], incomplete: true }) })
        expect(result.current.files).toEqual([])
        expect(result.current.loading).toBe(false)
        expect(searchWorkspaceFileNames).toHaveBeenCalledTimes(change === "active" ? 1 : 2)
    })

    it("does not expose completed results when query/active returns to its old value", async () => {
        searchWorkspaceFileNames.mockResolvedValue({ files: [file("/w/target")], incomplete: false })
        const { result, rerender } = renderHook(({ q, active }) => useFileNameSearch("/w", q, 0, active), { initialProps: { q: "target", active: true } })
        await tick()
        expect(result.current.files).toHaveLength(1)
        rerender({ q: "other", active: true }); rerender({ q: "target", active: true })
        expect(result.current).toMatchObject({ files: [], loading: true })
        await tick()
        rerender({ q: "target", active: false })
        expect(result.current).toMatchObject({ files: [], loading: false })
        rerender({ q: "target", active: true })
        expect(result.current).toMatchObject({ files: [], loading: true })
    })

    it("shares four legacy listing slots with cancelled generations", async () => {
        const old = Array.from({ length: 4 }, () => deferred<FileNode[]>())
        listDir.mockResolvedValueOnce(old.map((_, i) => dir(remote(`/w/${i}`))))
        for (const pending of old) listDir.mockReturnValueOnce(pending.promise)
        const { result, rerender } = renderHook(({ revision }) => useFileNameSearch(root, "target", revision, true), { initialProps: { revision: 0 } })
        await tick()
        expect(listDir).toHaveBeenCalledTimes(5)
        rerender({ revision: 1 }); await tick()
        expect(listDir).toHaveBeenCalledTimes(5)
        await act(async () => { old[0].resolve([dir(remote("/w/0/stale"))]) })
        expect(listDir).toHaveBeenCalledTimes(6)
        expect(result.current.loading).toBe(false)
        await act(async () => { old.slice(1).forEach((pending) => pending.resolve([])) })
        expect(listDir).toHaveBeenCalledTimes(6)
    })

    it("does not search for empty queries, missing roots, or inactive search", async () => {
        renderHook(() => useFileNameSearch("/w", "  ", 0, true))
        renderHook(() => useFileNameSearch(null, "target", 0, true))
        renderHook(() => useFileNameSearch("/w", "target", 0, false))
        await tick()
        expect(listDir).not.toHaveBeenCalled()
        expect(searchWorkspaceFileNames).not.toHaveBeenCalled()
        expect(searchRemoteFileNames).not.toHaveBeenCalled()
    })
})

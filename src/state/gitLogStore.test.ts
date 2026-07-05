import { afterEach, describe, expect, it, vi } from "vitest"

import type { AuthorEntry, CommitDetail, LogCommit, LogPage } from "../lib/types"

// Build a LogCommit with sensible defaults; override per test.
function mkCommit(hash: string, over: Partial<LogCommit> = {}): LogCommit {
    return {
        hash,
        shortHash: hash.slice(0, 7),
        subject: `subject ${hash}`,
        authorName: "Alice",
        authorEmail: "alice@example.com",
        timestamp: 1700000000,
        parents: [],
        refs: [],
        ...over
    }
}

function mkDetail(over: Partial<CommitDetail> = {}): CommitDetail {
    return {
        subject: "s",
        body: "b",
        authorName: "Alice",
        authorEmail: "alice@example.com",
        timestamp: 1700000000,
        parents: [],
        files: [],
        totalAdditions: 0,
        totalDeletions: 0,
        ...over
    }
}

const page = (commits: LogCommit[], hasMore: boolean): LogPage => ({ commits, hasMore })

vi.mock("../lib/ipc", () => ({
    gitLogPage: vi.fn(async () => page([mkCommit("a")], false)),
    gitCommitDetail: vi.fn(async () => mkDetail()),
    gitLogAuthors: vi.fn(async (): Promise<AuthorEntry[]> => [
        { name: "Alice", email: "alice@example.com" }
    ])
}))

describe("gitLogStore", () => {
    afterEach(async () => {
        vi.clearAllMocks()
        const { useGitLogStore } = await import("./gitLogStore")
        useGitLogStore.getState().reset()
    })

    it("loadFirstPage loads commits, hasMore, and authors", async () => {
        const { useGitLogStore } = await import("./gitLogStore")
        const ipc = await import("../lib/ipc")
        ;(ipc.gitLogPage as ReturnType<typeof vi.fn>).mockResolvedValueOnce(
            page([mkCommit("a"), mkCommit("b")], true)
        )
        await useGitLogStore.getState().loadFirstPage()
        const s = useGitLogStore.getState()
        expect(s.commits.map((c) => c.hash)).toEqual(["a", "b"])
        expect(s.hasMore).toBe(true)
        expect(s.loading).toBe(false)
        expect(s.authors).toEqual([{ name: "Alice", email: "alice@example.com" }])
        // skip=0, limit=LOG_PAGE_SIZE for the first page.
        expect(ipc.gitLogPage).toHaveBeenCalledWith(0, 200, null, null, null, null)
    })

    it("loadMore appends the next page and uses skip=current length", async () => {
        const { useGitLogStore } = await import("./gitLogStore")
        const ipc = await import("../lib/ipc")
        ;(ipc.gitLogPage as ReturnType<typeof vi.fn>)
            .mockResolvedValueOnce(page([mkCommit("a"), mkCommit("b")], true))
            .mockResolvedValueOnce(page([mkCommit("c")], false))
        await useGitLogStore.getState().loadFirstPage()
        await useGitLogStore.getState().loadMore()
        const s = useGitLogStore.getState()
        expect(s.commits.map((c) => c.hash)).toEqual(["a", "b", "c"])
        expect(s.hasMore).toBe(false)
        expect(ipc.gitLogPage).toHaveBeenLastCalledWith(2, 200, null, null, null, null)
    })

    it("loadMore is a no-op when hasMore is false", async () => {
        const { useGitLogStore } = await import("./gitLogStore")
        const ipc = await import("../lib/ipc")
        ;(ipc.gitLogPage as ReturnType<typeof vi.fn>).mockResolvedValueOnce(
            page([mkCommit("a")], false)
        )
        await useGitLogStore.getState().loadFirstPage()
        await useGitLogStore.getState().loadMore()
        // Only the first-page call happened; loadMore short-circuited.
        expect(ipc.gitLogPage).toHaveBeenCalledTimes(1)
    })

    it("setFilters merges and reloads the first page with the new filter", async () => {
        const { useGitLogStore } = await import("./gitLogStore")
        const ipc = await import("../lib/ipc")
        ;(ipc.gitLogPage as ReturnType<typeof vi.fn>).mockResolvedValue(page([mkCommit("x")], false))
        useGitLogStore.getState().setFilters({ query: "fix", author: "Alice" })
        await Promise.resolve()
        await Promise.resolve()
        expect(useGitLogStore.getState().filters).toMatchObject({ query: "fix", author: "Alice" })
        expect(ipc.gitLogPage).toHaveBeenLastCalledWith(0, 200, "fix", "Alice", null, null)
    })

    it("drops a stale first-page response when filters change mid-flight", async () => {
        const { useGitLogStore } = await import("./gitLogStore")
        const ipc = await import("../lib/ipc")
        // First (stale) call resolves slowly with old data; second call is fast.
        let releaseStale: (v: LogPage) => void = () => {}
        const stalePromise = new Promise<LogPage>((r) => {
            releaseStale = r
        })
        ;(ipc.gitLogPage as ReturnType<typeof vi.fn>)
            .mockReturnValueOnce(stalePromise)
            .mockResolvedValueOnce(page([mkCommit("fresh")], false))
        const first = useGitLogStore.getState().loadFirstPage()
        // Change filters → new generation, new fast response wins.
        useGitLogStore.getState().setFilters({ query: "new" })
        await Promise.resolve()
        await Promise.resolve()
        // Now let the stale response arrive late; it must be discarded.
        releaseStale(page([mkCommit("stale")], true))
        await first
        const s = useGitLogStore.getState()
        expect(s.commits.map((c) => c.hash)).toEqual(["fresh"])
        expect(s.hasMore).toBe(false)
    })

    it("overlapping reloads: stale round leaves loading true and loadMore stays gated", async () => {
        const { useGitLogStore } = await import("./gitLogStore")
        const ipc = await import("../lib/ipc")
        // Regression (reviewer round 2): two setFilters in a row put two reloads
        // in flight. When the OLD round returns first it must NOT touch
        // `loading` — the flag is owned by the newer round. Clearing it would
        // open the loadMore gate early: loadMore would fire with the old list
        // length as skip + the new filters, appending a wrong page.
        let releaseOld: (v: LogPage) => void = () => {}
        let releaseNew: (v: LogPage) => void = () => {}
        const oldRound = new Promise<LogPage>((r) => {
            releaseOld = r
        })
        const newRound = new Promise<LogPage>((r) => {
            releaseNew = r
        })
        ;(ipc.gitLogPage as ReturnType<typeof vi.fn>)
            .mockResolvedValueOnce(page([mkCommit("a"), mkCommit("b")], true)) // initial
            .mockReturnValueOnce(oldRound) // setFilters #1 reload
            .mockReturnValueOnce(newRound) // setFilters #2 reload
        await useGitLogStore.getState().loadFirstPage()
        useGitLogStore.getState().setFilters({ query: "x" })
        useGitLogStore.getState().setFilters({ query: "xy" })
        // Old round returns while the new round is still in flight → dropped,
        // and the flag must be left alone (still owned by the new round).
        releaseOld(page([mkCommit("stale")], true))
        await new Promise<void>((r) => setTimeout(r))
        expect(useGitLogStore.getState().loading).toBe(true)
        expect(useGitLogStore.getState().commits.map((c) => c.hash)).toEqual(["a", "b"])
        // Gate must be closed: loadMore is a no-op (no extra gitLogPage call).
        await useGitLogStore.getState().loadMore()
        expect(ipc.gitLogPage).toHaveBeenCalledTimes(3)
        // New round lands → its content wins and loading clears.
        releaseNew(page([mkCommit("f1")], false))
        await new Promise<void>((r) => setTimeout(r))
        const s = useGitLogStore.getState()
        expect(s.commits.map((c) => c.hash)).toEqual(["f1"])
        expect(s.loading).toBe(false)
    })

    it("loadMore stale drop releases loadingMore and later loadMore still works", async () => {
        const { useGitLogStore } = await import("./gitLogStore")
        const ipc = await import("../lib/ipc")
        // Regression (reviewer): a loadMore in flight when setFilters bumps the
        // generation must not leave `loadingMore` stuck true after its stale
        // page is dropped — that would gate-block every future loadMore.
        let releaseStale: (v: LogPage) => void = () => {}
        const stalePromise = new Promise<LogPage>((r) => {
            releaseStale = r
        })
        ;(ipc.gitLogPage as ReturnType<typeof vi.fn>)
            .mockResolvedValueOnce(page([mkCommit("a")], true)) // first page
            .mockReturnValueOnce(stalePromise) // loadMore, hangs in flight
            .mockResolvedValueOnce(page([mkCommit("f1")], true)) // setFilters reload
            .mockResolvedValueOnce(page([mkCommit("f2")], false)) // next loadMore
        await useGitLogStore.getState().loadFirstPage()
        const more = useGitLogStore.getState().loadMore()
        useGitLogStore.getState().setFilters({ query: "x" })
        // Let the reload settle while the stale loadMore is still in flight.
        await new Promise<void>((r) => setTimeout(r))
        expect(useGitLogStore.getState().commits.map((c) => c.hash)).toEqual(["f1"])
        // Stale page arrives late → dropped, and the flag must be released.
        releaseStale(page([mkCommit("stale")], true))
        await more
        const s = useGitLogStore.getState()
        expect(s.commits.map((c) => c.hash)).toEqual(["f1"]) // stale not appended
        expect(s.hasMore).toBe(true) // fresh page's hasMore kept
        expect(s.loadingMore).toBe(false) // regression: was stuck true
        // A subsequent legitimate loadMore must pass the gate and append.
        await useGitLogStore.getState().loadMore()
        expect(useGitLogStore.getState().commits.map((c) => c.hash)).toEqual(["f1", "f2"])
    })

    it("select fetches detail, caches it, and serves cache on repeat", async () => {
        const { useGitLogStore } = await import("./gitLogStore")
        const ipc = await import("../lib/ipc")
        ;(ipc.gitCommitDetail as ReturnType<typeof vi.fn>).mockResolvedValue(
            mkDetail({ subject: "the detail" })
        )
        await useGitLogStore.getState().select("h1")
        expect(useGitLogStore.getState().selectedHash).toBe("h1")
        expect(useGitLogStore.getState().detail?.subject).toBe("the detail")
        expect(useGitLogStore.getState().detailCache.has("h1")).toBe(true)
        // Second select of the same hash serves the cache — no extra fetch.
        await useGitLogStore.getState().select("h1")
        expect(ipc.gitCommitDetail).toHaveBeenCalledTimes(1)
    })

    it("select cache-miss clears the previous detail before the new one loads", async () => {
        const { useGitLogStore } = await import("./gitLogStore")
        const ipc = await import("../lib/ipc")
        // Select A first (loads + caches detail A).
        ;(ipc.gitCommitDetail as ReturnType<typeof vi.fn>).mockResolvedValueOnce(
            mkDetail({ subject: "detail A" })
        )
        await useGitLogStore.getState().select("A")
        expect(useGitLogStore.getState().detail?.subject).toBe("detail A")

        // Now select B (cache miss). B's detail hangs in flight so we can observe
        // the window: detail must be cleared immediately (not still showing A) and
        // detailLoading must be true.
        let releaseB: (d: CommitDetail) => void = () => {}
        const bPromise = new Promise<CommitDetail>((r) => {
            releaseB = r
        })
        ;(ipc.gitCommitDetail as ReturnType<typeof vi.fn>).mockReturnValueOnce(bPromise)
        const selecting = useGitLogStore.getState().select("B")
        // Synchronous window right after selection changed: no stale A.
        expect(useGitLogStore.getState().selectedHash).toBe("B")
        expect(useGitLogStore.getState().detail).toBe(null)
        expect(useGitLogStore.getState().detailLoading).toBe(true)
        // B resolves → detail B shown, loading cleared.
        releaseB(mkDetail({ subject: "detail B" }))
        await selecting
        expect(useGitLogStore.getState().detail?.subject).toBe("detail B")
        expect(useGitLogStore.getState().detailLoading).toBe(false)
    })

    it("select cache-hit keeps detail set synchronously (no clear flash)", async () => {
        const { useGitLogStore } = await import("./gitLogStore")
        const ipc = await import("../lib/ipc")
        ;(ipc.gitCommitDetail as ReturnType<typeof vi.fn>).mockResolvedValue(
            mkDetail({ subject: "cached" })
        )
        // Prime the cache for "A".
        await useGitLogStore.getState().select("A")
        // Move away, then back to A (now a cache hit): detail must be A the whole
        // time — the cache path must NOT clear detail.
        await useGitLogStore.getState().select("A")
        expect(useGitLogStore.getState().detail?.subject).toBe("cached")
        expect(useGitLogStore.getState().detailLoading).toBe(false)
    })

    it("detail cache evicts the oldest entry past the cap", async () => {
        const { useGitLogStore, DETAIL_CACHE_LIMIT } = await import("./gitLogStore")
        const ipc = await import("../lib/ipc")
        ;(ipc.gitCommitDetail as ReturnType<typeof vi.fn>).mockImplementation(async () => mkDetail())
        for (let i = 0; i < DETAIL_CACHE_LIMIT + 1; i++) {
            await useGitLogStore.getState().select(`h${i}`)
        }
        const cache = useGitLogStore.getState().detailCache
        expect(cache.size).toBe(DETAIL_CACHE_LIMIT)
        expect(cache.has("h0")).toBe(false) // oldest evicted
        expect(cache.has(`h${DETAIL_CACHE_LIMIT}`)).toBe(true) // newest kept
    })

    it("select records error and clears detailLoading on IPC failure", async () => {
        const { useGitLogStore } = await import("./gitLogStore")
        const ipc = await import("../lib/ipc")
        ;(ipc.gitCommitDetail as ReturnType<typeof vi.fn>).mockRejectedValueOnce(
            new Error("no such object")
        )
        await useGitLogStore.getState().select("bad")
        expect(useGitLogStore.getState().error).toContain("no such object")
        expect(useGitLogStore.getState().detailLoading).toBe(false)
    })

    it("loadFirstPage records error and does not crash on IPC failure", async () => {
        const { useGitLogStore } = await import("./gitLogStore")
        const ipc = await import("../lib/ipc")
        ;(ipc.gitLogPage as ReturnType<typeof vi.fn>).mockRejectedValueOnce(new Error("git boom"))
        await useGitLogStore.getState().loadFirstPage()
        const s = useGitLogStore.getState()
        expect(s.error).toContain("git boom")
        expect(s.loading).toBe(false)
        expect(s.commits).toEqual([])
    })

    it("reset restores initial state and drops selection/detail", async () => {
        const { useGitLogStore } = await import("./gitLogStore")
        await useGitLogStore.getState().loadFirstPage()
        await useGitLogStore.getState().select("h1")
        useGitLogStore.getState().reset()
        const s = useGitLogStore.getState()
        expect(s.commits).toEqual([])
        expect(s.selectedHash).toBe(null)
        expect(s.detail).toBe(null)
        expect(s.detailCache.size).toBe(0)
        expect(s.filters).toEqual({ query: "", author: null, since: null, until: null })
    })
})

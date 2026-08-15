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

const page = (commits: LogCommit[], hasMore: boolean, nextCursor: string | null = hasMore ? "cursor-next" : null): LogPage => ({ commits, hasMore, nextCursor })

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
        await useGitLogStore.getState().loadFirstPage("/w")
        const s = useGitLogStore.getState()
        expect(s.commits.map((c) => c.hash)).toEqual(["a", "b"])
        expect(s.hasMore).toBe(true)
        expect(s.loading).toBe(false)
        expect(s.authors).toEqual([{ name: "Alice", email: "alice@example.com" }])
        // skip=0, limit=LOG_PAGE_SIZE for the first page.
        expect(ipc.gitLogPage).toHaveBeenCalledWith("/w", null, 200, null, null, null, null)
    })

    it("does not rescan authors on same-root first-page reloads or filter changes", async () => {
        const { useGitLogStore } = await import("./gitLogStore")
        const ipc = await import("../lib/ipc")
        ;(ipc.gitLogPage as ReturnType<typeof vi.fn>).mockResolvedValue(page([mkCommit("a")], false))
        await useGitLogStore.getState().loadFirstPage("/w")
        expect(ipc.gitLogAuthors).toHaveBeenCalledTimes(1)
        await useGitLogStore.getState().loadFirstPage("/w")
        useGitLogStore.getState().setFilters("/w", { query: "fix" })
        await Promise.resolve()
        await Promise.resolve()
        expect(ipc.gitLogAuthors).toHaveBeenCalledTimes(1)
        await useGitLogStore.getState().loadFirstPage("/other")
        expect(ipc.gitLogAuthors).toHaveBeenCalledTimes(2)
        expect(ipc.gitLogAuthors).toHaveBeenLastCalledWith("/other")
    })

    it("reuses resolved authors after A → B → A without a rescan", async () => {
        const { useGitLogStore } = await import("./gitLogStore")
        const ipc = await import("../lib/ipc")
        ;(ipc.gitLogPage as ReturnType<typeof vi.fn>).mockResolvedValue(page([mkCommit("x")], false))
        ;(ipc.gitLogAuthors as ReturnType<typeof vi.fn>)
            .mockResolvedValueOnce([{ name: "Alice", email: "a@x" }])
            .mockResolvedValueOnce([{ name: "Bob", email: "b@x" }])
        await useGitLogStore.getState().loadFirstPage("/repo-a")
        expect(useGitLogStore.getState().authors).toEqual([{ name: "Alice", email: "a@x" }])
        await useGitLogStore.getState().loadFirstPage("/repo-b")
        expect(useGitLogStore.getState().authors).toEqual([{ name: "Bob", email: "b@x" }])
        await useGitLogStore.getState().loadFirstPage("/repo-a")
        expect(ipc.gitLogAuthors).toHaveBeenCalledTimes(2)
        expect(useGitLogStore.getState().authors).toEqual([{ name: "Alice", email: "a@x" }])
    })

    it("preserve-cache reset clears displayed authors but reuses A after A → B → A", async () => {
        const { useGitLogStore } = await import("./gitLogStore")
        const ipc = await import("../lib/ipc")
        ;(ipc.gitLogPage as ReturnType<typeof vi.fn>).mockResolvedValue(page([mkCommit("x")], false))
        ;(ipc.gitLogAuthors as ReturnType<typeof vi.fn>)
            .mockResolvedValueOnce([{ name: "Alice", email: "a@x" }])
            .mockResolvedValueOnce([{ name: "Bob", email: "b@x" }])
        await useGitLogStore.getState().loadFirstPage("/repo-a")
        expect(useGitLogStore.getState().authors).toEqual([{ name: "Alice", email: "a@x" }])

        useGitLogStore.getState().reset({ preserveAuthorSession: true })
        expect(useGitLogStore.getState().authors).toEqual([])
        expect(useGitLogStore.getState().commits).toEqual([])

        await useGitLogStore.getState().loadFirstPage("/repo-b")
        expect(useGitLogStore.getState().authors).toEqual([{ name: "Bob", email: "b@x" }])
        useGitLogStore.getState().reset({ preserveAuthorSession: true })
        expect(useGitLogStore.getState().authors).toEqual([])

        await useGitLogStore.getState().loadFirstPage("/repo-a")
        expect(ipc.gitLogAuthors).toHaveBeenCalledTimes(2)
        expect(useGitLogStore.getState().authors).toEqual([{ name: "Alice", email: "a@x" }])
    })

    it("full reset clears the author cache so a later same-root load rescans", async () => {
        const { useGitLogStore } = await import("./gitLogStore")
        const ipc = await import("../lib/ipc")
        ;(ipc.gitLogPage as ReturnType<typeof vi.fn>).mockResolvedValue(page([mkCommit("x")], false))
        ;(ipc.gitLogAuthors as ReturnType<typeof vi.fn>)
            .mockResolvedValueOnce([{ name: "Alice", email: "a@x" }])
            .mockResolvedValueOnce([{ name: "Alice2", email: "a2@x" }])
        await useGitLogStore.getState().loadFirstPage("/repo-a")
        expect(ipc.gitLogAuthors).toHaveBeenCalledTimes(1)
        useGitLogStore.getState().reset()
        expect(useGitLogStore.getState().authors).toEqual([])
        await useGitLogStore.getState().loadFirstPage("/repo-a")
        expect(ipc.gitLogAuthors).toHaveBeenCalledTimes(2)
        expect(useGitLogStore.getState().authors).toEqual([{ name: "Alice2", email: "a2@x" }])
    })

    it("preserve-cache reset still drops a late A authors publish after switching to B", async () => {
        const { useGitLogStore } = await import("./gitLogStore")
        const ipc = await import("../lib/ipc")
        ;(ipc.gitLogPage as ReturnType<typeof vi.fn>).mockResolvedValue(page([mkCommit("x")], false))
        let resolveA!: (value: AuthorEntry[]) => void
        ;(ipc.gitLogAuthors as ReturnType<typeof vi.fn>)
            .mockImplementationOnce(() => new Promise<AuthorEntry[]>((resolve) => { resolveA = resolve }))
            .mockResolvedValueOnce([{ name: "Bob", email: "b@x" }])
        const pendingA = useGitLogStore.getState().loadFirstPage("/repo-a")
        useGitLogStore.getState().reset({ preserveAuthorSession: true })
        expect(useGitLogStore.getState().authors).toEqual([])
        await useGitLogStore.getState().loadFirstPage("/repo-b")
        expect(useGitLogStore.getState().authors).toEqual([{ name: "Bob", email: "b@x" }])
        resolveA([{ name: "Alice", email: "a@x" }])
        await pendingA
        expect(useGitLogStore.getState().authors).toEqual([{ name: "Bob", email: "b@x" }])
        useGitLogStore.getState().reset({ preserveAuthorSession: true })
        await useGitLogStore.getState().loadFirstPage("/repo-a")
        expect(ipc.gitLogAuthors).toHaveBeenCalledTimes(2)
        expect(useGitLogStore.getState().authors).toEqual([{ name: "Alice", email: "a@x" }])
    })

    it("clears stale authors immediately when switching to an uncached root", async () => {
        const { useGitLogStore } = await import("./gitLogStore")
        const ipc = await import("../lib/ipc")
        ;(ipc.gitLogPage as ReturnType<typeof vi.fn>).mockResolvedValue(page([mkCommit("x")], false))
        await useGitLogStore.getState().loadFirstPage("/repo-a")
        expect(useGitLogStore.getState().authors).toEqual([{ name: "Alice", email: "alice@example.com" }])
        let resolveB!: (value: AuthorEntry[]) => void
        ;(ipc.gitLogAuthors as ReturnType<typeof vi.fn>).mockImplementationOnce(
            () => new Promise<AuthorEntry[]>((resolve) => { resolveB = resolve })
        )
        const pending = useGitLogStore.getState().loadFirstPage("/repo-b")
        expect(useGitLogStore.getState().authors).toEqual([])
        resolveB([{ name: "Bob", email: "bob@example.com" }])
        await pending
        expect(useGitLogStore.getState().authors).toEqual([{ name: "Bob", email: "bob@example.com" }])
    })

    it("retries authors after a failed request for the same root", async () => {
        const { useGitLogStore } = await import("./gitLogStore")
        const ipc = await import("../lib/ipc")
        ;(ipc.gitLogPage as ReturnType<typeof vi.fn>).mockResolvedValue(page([mkCommit("x")], false))
        ;(ipc.gitLogAuthors as ReturnType<typeof vi.fn>)
            .mockRejectedValueOnce(new Error("authors down"))
            .mockResolvedValueOnce([{ name: "Retry", email: "r@x" }])
        await useGitLogStore.getState().loadFirstPage("/repo-b")
        expect(useGitLogStore.getState().authors).toEqual([])
        await useGitLogStore.getState().loadFirstPage("/repo-b")
        expect(ipc.gitLogAuthors).toHaveBeenCalledTimes(2)
        expect(useGitLogStore.getState().authors).toEqual([{ name: "Retry", email: "r@x" }])
    })

    it("ignores a late A authors result after B has become current", async () => {
        const { useGitLogStore } = await import("./gitLogStore")
        const ipc = await import("../lib/ipc")
        ;(ipc.gitLogPage as ReturnType<typeof vi.fn>).mockResolvedValue(page([mkCommit("x")], false))
        let resolveA!: (value: AuthorEntry[]) => void
        let resolveB!: (value: AuthorEntry[]) => void
        ;(ipc.gitLogAuthors as ReturnType<typeof vi.fn>)
            .mockImplementationOnce(() => new Promise<AuthorEntry[]>((resolve) => { resolveA = resolve }))
            .mockImplementationOnce(() => new Promise<AuthorEntry[]>((resolve) => { resolveB = resolve }))
        const pendingA = useGitLogStore.getState().loadFirstPage("/repo-a")
        const pendingB = useGitLogStore.getState().loadFirstPage("/repo-b")
        resolveB([{ name: "Bob", email: "b@x" }])
        await pendingB
        expect(useGitLogStore.getState().authors).toEqual([{ name: "Bob", email: "b@x" }])
        resolveA([{ name: "Alice", email: "a@x" }])
        await pendingA
        expect(useGitLogStore.getState().authors).toEqual([{ name: "Bob", email: "b@x" }])
    })

    it("loadMore appends the next page and uses the opaque nextCursor", async () => {
        const { useGitLogStore } = await import("./gitLogStore")
        const ipc = await import("../lib/ipc")
        ;(ipc.gitLogPage as ReturnType<typeof vi.fn>)
            .mockResolvedValueOnce(page([mkCommit("a"), mkCommit("b")], true))
            .mockResolvedValueOnce(page([mkCommit("c")], false))
        await useGitLogStore.getState().loadFirstPage("/w")
        await useGitLogStore.getState().loadMore("/w")
        const s = useGitLogStore.getState()
        expect(s.commits.map((c) => c.hash)).toEqual(["a", "b", "c"])
        expect(s.hasMore).toBe(false)
        expect(ipc.gitLogPage).toHaveBeenLastCalledWith("/w", "cursor-next", 200, null, null, null, null)
    })

    it("loadMore drops hashes already loaded (defensive dedup)", async () => {
        // Cursor pages are tip-stable; still dedup so a defensive overlap never
        // produces duplicate hash-keyed React rows.
        const { useGitLogStore } = await import("./gitLogStore")
        const ipc = await import("../lib/ipc")
        ;(ipc.gitLogPage as ReturnType<typeof vi.fn>)
            .mockResolvedValueOnce(page([mkCommit("a"), mkCommit("b")], true))
            .mockResolvedValueOnce(page([mkCommit("b"), mkCommit("c")], false))
        await useGitLogStore.getState().loadFirstPage("/w")
        await useGitLogStore.getState().loadMore("/w")
        expect(
            useGitLogStore.getState().commits.map((c) => c.hash)
        ).toEqual(["a", "b", "c"])
    })

    it("loadMore is a no-op when hasMore is false", async () => {
        const { useGitLogStore } = await import("./gitLogStore")
        const ipc = await import("../lib/ipc")
        ;(ipc.gitLogPage as ReturnType<typeof vi.fn>).mockResolvedValueOnce(
            page([mkCommit("a")], false)
        )
        await useGitLogStore.getState().loadFirstPage("/w")
        await useGitLogStore.getState().loadMore("/w")
        // Only the first-page call happened; loadMore short-circuited.
        expect(ipc.gitLogPage).toHaveBeenCalledTimes(1)
    })

    it("setFilters merges and reloads the first page with the new filter", async () => {
        const { useGitLogStore } = await import("./gitLogStore")
        const ipc = await import("../lib/ipc")
        ;(ipc.gitLogPage as ReturnType<typeof vi.fn>).mockResolvedValue(page([mkCommit("x")], false))
        useGitLogStore.getState().setFilters("/w", { query: "fix", author: "Alice" })
        await Promise.resolve()
        await Promise.resolve()
        expect(useGitLogStore.getState().filters).toMatchObject({ query: "fix", author: "Alice" })
        expect(ipc.gitLogPage).toHaveBeenLastCalledWith("/w", null, 200, "fix", "Alice", null, null)
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
        const first = useGitLogStore.getState().loadFirstPage("/w")
        // Change filters → new generation, new fast response wins.
        useGitLogStore.getState().setFilters("/w", { query: "new" })
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
        await useGitLogStore.getState().loadFirstPage("/w")
        useGitLogStore.getState().setFilters("/w", { query: "x" })
        useGitLogStore.getState().setFilters("/w", { query: "xy" })
        // Old round returns while the new round is still in flight → dropped,
        // and the flag must be left alone (still owned by the new round).
        releaseOld(page([mkCommit("stale")], true))
        await new Promise<void>((r) => setTimeout(r))
        expect(useGitLogStore.getState().loading).toBe(true)
        expect(useGitLogStore.getState().commits.map((c) => c.hash)).toEqual(["a", "b"])
        // Gate must be closed: loadMore is a no-op (no extra gitLogPage call).
        await useGitLogStore.getState().loadMore("/w")
        expect(ipc.gitLogPage).toHaveBeenCalledTimes(3)
        // New round lands → its content wins and loading clears.
        releaseNew(page([mkCommit("f1")], false))
        await new Promise<void>((r) => setTimeout(r))
        const s = useGitLogStore.getState()
        expect(s.commits.map((c) => c.hash)).toEqual(["f1"])
        expect(s.loading).toBe(false)
    })

    it("loadMore stale drop is owned by loadFirstPage and later loadMore still works", async () => {
        const { useGitLogStore } = await import("./gitLogStore")
        const ipc = await import("../lib/ipc")
        // loadFirstPage/setFilters owns loadingMore for the new epoch; a late
        // loadMore resolve must write nothing and must not re-open the gate
        // incorrectly or leave it stuck.
        let releaseStale: (v: LogPage) => void = () => {}
        const stalePromise = new Promise<LogPage>((r) => {
            releaseStale = r
        })
        ;(ipc.gitLogPage as ReturnType<typeof vi.fn>)
            .mockResolvedValueOnce(page([mkCommit("a")], true)) // first page
            .mockReturnValueOnce(stalePromise) // loadMore, hangs in flight
            .mockResolvedValueOnce(page([mkCommit("f1")], true)) // setFilters reload
            .mockResolvedValueOnce(page([mkCommit("f2")], false)) // next loadMore
        await useGitLogStore.getState().loadFirstPage("/w")
        const more = useGitLogStore.getState().loadMore("/w")
        useGitLogStore.getState().setFilters("/w", { query: "x" })
        // Superseding first page clears loadingMore synchronously.
        expect(useGitLogStore.getState().loadingMore).toBe(false)
        // Let the reload settle while the stale loadMore is still in flight.
        await new Promise<void>((r) => setTimeout(r))
        expect(useGitLogStore.getState().commits.map((c) => c.hash)).toEqual(["f1"])
        // Stale page arrives late → dropped with zero writes.
        releaseStale(page([mkCommit("stale")], true))
        await more
        const s = useGitLogStore.getState()
        expect(s.commits.map((c) => c.hash)).toEqual(["f1"]) // stale not appended
        expect(s.hasMore).toBe(true) // fresh page's hasMore kept
        expect(s.loadingMore).toBe(false)
        // A subsequent legitimate loadMore must pass the gate and append.
        await useGitLogStore.getState().loadMore("/w")
        expect(useGitLogStore.getState().commits.map((c) => c.hash)).toEqual(["f1", "f2"])
    })

    it("stale A loadMore never clears B loadingMore after root switch", async () => {
        const { useGitLogStore } = await import("./gitLogStore")
        const ipc = await import("../lib/ipc")
        let releaseA: (v: LogPage) => void = () => {}
        let releaseB: (v: LogPage) => void = () => {}
        const aMore = new Promise<LogPage>((resolve) => {
            releaseA = resolve
        })
        const bMore = new Promise<LogPage>((resolve) => {
            releaseB = resolve
        })
        ;(ipc.gitLogPage as ReturnType<typeof vi.fn>)
            .mockResolvedValueOnce(page([mkCommit("a1")], true)) // A first page
            .mockReturnValueOnce(aMore) // A loadMore pending
            .mockResolvedValueOnce(page([mkCommit("b1")], true)) // B first page
            .mockReturnValueOnce(bMore) // B loadMore pending

        await useGitLogStore.getState().loadFirstPage("/a")
        const pendingA = useGitLogStore.getState().loadMore("/a")
        expect(useGitLogStore.getState().loadingMore).toBe(true)

        useGitLogStore.getState().reset()
        await useGitLogStore.getState().loadFirstPage("/b")
        expect(useGitLogStore.getState().loadingMore).toBe(false)
        expect(useGitLogStore.getState().commits.map((c) => c.hash)).toEqual(["b1"])

        const pendingB = useGitLogStore.getState().loadMore("/b")
        expect(useGitLogStore.getState().loadingMore).toBe(true)
        const callsWhileBPending = (ipc.gitLogPage as ReturnType<typeof vi.fn>).mock.calls.length

        // Resolve stale A: must not clear B's loadingMore or mutate B state.
        releaseA(page([mkCommit("a-stale")], true))
        await pendingA
        expect(useGitLogStore.getState().loadingMore).toBe(true)
        expect(useGitLogStore.getState().commits.map((c) => c.hash)).toEqual(["b1"])
        // Gate still held — concurrent loadMore is a no-op.
        await useGitLogStore.getState().loadMore("/b")
        expect((ipc.gitLogPage as ReturnType<typeof vi.fn>).mock.calls.length).toBe(callsWhileBPending)

        releaseB(page([mkCommit("b2")], false))
        await pendingB
        expect(useGitLogStore.getState().loadingMore).toBe(false)
        expect(useGitLogStore.getState().commits.map((c) => c.hash)).toEqual(["b1", "b2"])
    })

    it("stale A loadMore reject never clears B loadingMore after root switch", async () => {
        const { useGitLogStore } = await import("./gitLogStore")
        const ipc = await import("../lib/ipc")
        let rejectA: (e: Error) => void = () => {}
        let releaseB: (v: LogPage) => void = () => {}
        const aMore = new Promise<LogPage>((_resolve, reject) => {
            rejectA = reject
        })
        const bMore = new Promise<LogPage>((resolve) => {
            releaseB = resolve
        })
        ;(ipc.gitLogPage as ReturnType<typeof vi.fn>)
            .mockResolvedValueOnce(page([mkCommit("a1")], true))
            .mockReturnValueOnce(aMore)
            .mockResolvedValueOnce(page([mkCommit("b1")], true))
            .mockReturnValueOnce(bMore)

        await useGitLogStore.getState().loadFirstPage("/a")
        const pendingA = useGitLogStore.getState().loadMore("/a")
        useGitLogStore.getState().reset()
        await useGitLogStore.getState().loadFirstPage("/b")
        const pendingB = useGitLogStore.getState().loadMore("/b")
        expect(useGitLogStore.getState().loadingMore).toBe(true)

        rejectA(new Error("stale A failed"))
        // loadMore swallows the rejection after the root/epoch check.
        await pendingA
        expect(useGitLogStore.getState().loadingMore).toBe(true)
        expect(useGitLogStore.getState().listError).toBeNull()
        expect(useGitLogStore.getState().loadMoreError).toBeNull()
        expect(useGitLogStore.getState().detailError).toBeNull()
        expect(useGitLogStore.getState().commits.map((c) => c.hash)).toEqual(["b1"])

        releaseB(page([mkCommit("b2")], false))
        await pendingB
        expect(useGitLogStore.getState().loadingMore).toBe(false)
        expect(useGitLogStore.getState().commits.map((c) => c.hash)).toEqual(["b1", "b2"])
    })

    it("select fetches detail, caches it, and serves cache on repeat", async () => {
        const { useGitLogStore } = await import("./gitLogStore")
        const ipc = await import("../lib/ipc")
        ;(ipc.gitCommitDetail as ReturnType<typeof vi.fn>).mockResolvedValue(
            mkDetail({ subject: "the detail" })
        )
        await useGitLogStore.getState().select("/w", "h1")
        expect(useGitLogStore.getState().selectedHash).toBe("h1")
        expect(useGitLogStore.getState().detail?.subject).toBe("the detail")
        expect(useGitLogStore.getState().detailCache.has("/w\0h1")).toBe(true)
        // Second select of the same hash serves the cache — no extra fetch.
        await useGitLogStore.getState().select("/w", "h1")
        expect(ipc.gitCommitDetail).toHaveBeenCalledTimes(1)
    })

    it("select cache-miss clears the previous detail before the new one loads", async () => {
        const { useGitLogStore } = await import("./gitLogStore")
        const ipc = await import("../lib/ipc")
        // Select A first (loads + caches detail A).
        ;(ipc.gitCommitDetail as ReturnType<typeof vi.fn>).mockResolvedValueOnce(
            mkDetail({ subject: "detail A" })
        )
        await useGitLogStore.getState().select("/w", "A")
        expect(useGitLogStore.getState().detail?.subject).toBe("detail A")

        // Now select B (cache miss). B's detail hangs in flight so we can observe
        // the window: detail must be cleared immediately (not still showing A) and
        // detailLoading must be true.
        let releaseB: (d: CommitDetail) => void = () => {}
        const bPromise = new Promise<CommitDetail>((r) => {
            releaseB = r
        })
        ;(ipc.gitCommitDetail as ReturnType<typeof vi.fn>).mockReturnValueOnce(bPromise)
        const selecting = useGitLogStore.getState().select("/w", "B")
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
        await useGitLogStore.getState().select("/w", "A")
        // Move away, then back to A (now a cache hit): detail must be A the whole
        // time — the cache path must NOT clear detail.
        await useGitLogStore.getState().select("/w", "A")
        expect(useGitLogStore.getState().detail?.subject).toBe("cached")
        expect(useGitLogStore.getState().detailLoading).toBe(false)
    })

    it("detail cache evicts the oldest entry past the cap", async () => {
        const { useGitLogStore, DETAIL_CACHE_LIMIT } = await import("./gitLogStore")
        const ipc = await import("../lib/ipc")
        ;(ipc.gitCommitDetail as ReturnType<typeof vi.fn>).mockImplementation(async () => mkDetail())
        for (let i = 0; i < DETAIL_CACHE_LIMIT + 1; i++) {
            await useGitLogStore.getState().select("/w", `h${i}`)
        }
        const cache = useGitLogStore.getState().detailCache
        expect(cache.size).toBe(DETAIL_CACHE_LIMIT)
        expect(cache.has(`/w\0h0`)).toBe(false) // oldest evicted
        expect(cache.has(`/w\0h${DETAIL_CACHE_LIMIT}`)).toBe(true) // newest kept
    })

    it("select records detailError and clears detailLoading on IPC failure", async () => {
        const { useGitLogStore } = await import("./gitLogStore")
        const ipc = await import("../lib/ipc")
        ;(ipc.gitCommitDetail as ReturnType<typeof vi.fn>).mockRejectedValueOnce(
            new Error("no such object")
        )
        await useGitLogStore.getState().select("/w", "bad")
        expect(useGitLogStore.getState().detailError).toContain("no such object")
        expect(useGitLogStore.getState().listError).toBeNull()
        expect(useGitLogStore.getState().detailLoading).toBe(false)
    })

    it("loadFirstPage records listError and does not crash on IPC failure", async () => {
        const { useGitLogStore } = await import("./gitLogStore")
        const ipc = await import("../lib/ipc")
        ;(ipc.gitLogPage as ReturnType<typeof vi.fn>).mockRejectedValueOnce(new Error("git boom"))
        await useGitLogStore.getState().loadFirstPage("/w")
        const s = useGitLogStore.getState()
        expect(s.listError).toContain("git boom")
        expect(s.detailError).toBeNull()
        expect(s.loading).toBe(false)
        expect(s.commits).toEqual([])
    })

    it("loadMore failure preserves loaded commits and sets loadMoreError only", async () => {
        const { useGitLogStore } = await import("./gitLogStore")
        const ipc = await import("../lib/ipc")
        ;(ipc.gitLogPage as ReturnType<typeof vi.fn>)
            .mockResolvedValueOnce(page([mkCommit("a")], true, "c1"))
            .mockRejectedValueOnce(new Error("page 2 boom"))
        await useGitLogStore.getState().loadFirstPage("/w")
        await useGitLogStore.getState().loadMore("/w")
        const s = useGitLogStore.getState()
        expect(s.commits.map((c) => c.hash)).toEqual(["a"])
        expect(s.loadMoreError).toContain("page 2 boom")
        expect(s.listError).toBeNull()
        expect(s.loadingMore).toBe(false)
        expect(s.hasMore).toBe(true)
    })

    it("reset restores initial state and drops selection/detail", async () => {
        const { useGitLogStore } = await import("./gitLogStore")
        await useGitLogStore.getState().loadFirstPage("/w")
        await useGitLogStore.getState().select("/w", "h1")
        useGitLogStore.getState().reset()
        const s = useGitLogStore.getState()
        expect(s.commits).toEqual([])
        expect(s.selectedHash).toBe(null)
        expect(s.detail).toBe(null)
        expect(s.detailCache.size).toBe(0)
        expect(s.filters).toEqual({ query: "", author: null, since: null, until: null })
    })

    it("drops in-flight A page/authors/detail after B reset", async () => {
        const { useGitLogStore } = await import("./gitLogStore")
        const ipc = await import("../lib/ipc")

        let resolveAPage!: (value: LogPage) => void
        let resolveAAuthors!: (value: AuthorEntry[]) => void
        let resolveADetail!: (value: CommitDetail) => void

        ;(ipc.gitLogPage as ReturnType<typeof vi.fn>).mockImplementationOnce(
            () => new Promise<LogPage>((resolve) => { resolveAPage = resolve })
        )
        ;(ipc.gitLogAuthors as ReturnType<typeof vi.fn>).mockImplementationOnce(
            () => new Promise<AuthorEntry[]>((resolve) => { resolveAAuthors = resolve })
        )
        ;(ipc.gitCommitDetail as ReturnType<typeof vi.fn>).mockImplementationOnce(
            () => new Promise<CommitDetail>((resolve) => { resolveADetail = resolve })
        )

        const pagePromise = useGitLogStore.getState().loadFirstPage("/repo-a")
        const detailPromise = useGitLogStore.getState().select("/repo-a", "deadbeef")

        // Switch repository epoch before A resolves.
        useGitLogStore.getState().reset()
        ;(ipc.gitLogPage as ReturnType<typeof vi.fn>).mockResolvedValueOnce(
            page([mkCommit("b1")], false)
        )
        ;(ipc.gitLogAuthors as ReturnType<typeof vi.fn>).mockResolvedValueOnce([
            { name: "Bob", email: "bob@example.com" }
        ])
        await useGitLogStore.getState().loadFirstPage("/repo-b")

        resolveAPage(page([mkCommit("a1")], false))
        resolveAAuthors([{ name: "Alice", email: "alice@example.com" }])
        resolveADetail(mkDetail({ subject: "from A" }))
        await pagePromise
        await detailPromise

        const s = useGitLogStore.getState()
        expect(s.commits.map((c) => c.hash)).toEqual(["b1"])
        expect(s.authors).toEqual([{ name: "Bob", email: "bob@example.com" }])
        expect(s.detail).toBe(null)
        expect(s.selectedHash).toBe(null)
    })

    it("detail cache is scoped by repository root", async () => {
        const { useGitLogStore } = await import("./gitLogStore")
        const ipc = await import("../lib/ipc")
        ;(ipc.gitCommitDetail as ReturnType<typeof vi.fn>)
            .mockResolvedValueOnce(mkDetail({ subject: "from A" }))
            .mockResolvedValueOnce(mkDetail({ subject: "from B" }))
        await useGitLogStore.getState().select("/a", "H")
        expect(useGitLogStore.getState().detail?.subject).toBe("from A")
        // Same hash under a different root must not serve A's cache.
        await useGitLogStore.getState().select("/b", "H")
        expect(ipc.gitCommitDetail).toHaveBeenCalledWith("/b", "H")
        expect(useGitLogStore.getState().detail?.subject).toBe("from B")
    })

    it("list reload settles detailLoading so a stale detail cannot stick the spinner", async () => {
        const { useGitLogStore } = await import("./gitLogStore")
        const ipc = await import("../lib/ipc")
        let releaseDetail: (d: CommitDetail) => void = () => {}
        const pending = new Promise<CommitDetail>((r) => {
            releaseDetail = r
        })
        ;(ipc.gitCommitDetail as ReturnType<typeof vi.fn>).mockReturnValueOnce(pending)
        ;(ipc.gitLogPage as ReturnType<typeof vi.fn>).mockResolvedValue(
            page([mkCommit("a")], false)
        )
        const selecting = useGitLogStore.getState().select("/w", "a")
        expect(useGitLogStore.getState().detailLoading).toBe(true)
        // Filter/reload bumps generation and must clear detail ownership.
        useGitLogStore.getState().setFilters("/w", { query: "x" })
        await Promise.resolve()
        await Promise.resolve()
        expect(useGitLogStore.getState().detailLoading).toBe(false)
        expect(useGitLogStore.getState().selectedHash).toBeNull()
        // Late resolve must write nothing.
        releaseDetail(mkDetail({ subject: "stale" }))
        await selecting
        expect(useGitLogStore.getState().detail).toBeNull()
        expect(useGitLogStore.getState().detailLoading).toBe(false)
    })

})

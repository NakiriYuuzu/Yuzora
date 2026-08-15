import { create } from "zustand"

import { gitCommitDetail, gitLogAuthors, gitLogPage } from "../lib/ipc"
import type { AuthorEntry, CommitDetail, LogCommit } from "../lib/types"

// Page size for the commit list. One request loads this many commits; loadMore
// appends another page. 200 matches the design's "load a healthy window, page on
// scroll" intent without overwhelming the graph layout.
const LOG_PAGE_SIZE = 200

// Commit-detail cache cap. A simple bounded map (not a strict LRU): once full we
// drop the oldest-inserted entry. Detail payloads are cheap and re-fetchable, so
// eviction accuracy doesn't matter — only unbounded growth does.
export const DETAIL_CACHE_LIMIT = 50

interface LogFilters {
    query: string
    author: string | null
    since: string | null
    until: string | null
}

const initialFilters: LogFilters = { query: "", author: null, since: null, until: null }

interface GitLogState {
    commits: LogCommit[]
    hasMore: boolean
    /** Opaque backend cursor for the next page; null when exhausted / unset. */
    nextCursor: string | null
    loading: boolean // first page load
    loadingMore: boolean // pagination load
    listError: string | null
    loadMoreError: string | null
    detailError: string | null
    filters: LogFilters
    authors: AuthorEntry[]
    selectedHash: string | null
    detail: CommitDetail | null
    detailLoading: boolean
    detailCache: Map<string, CommitDetail>

    loadFirstPage: (repositoryRoot: string) => Promise<void>
    loadMore: (repositoryRoot: string) => Promise<void>
    setFilters: (repositoryRoot: string, partial: Partial<LogFilters>) => void
    select: (repositoryRoot: string, hash: string) => Promise<void>
    reset: (options?: { preserveAuthorSession?: boolean }) => void
}

const initialGitLogState = {
    commits: [] as LogCommit[],
    hasMore: false,
    nextCursor: null as string | null,
    loading: false,
    loadingMore: false,
    listError: null as string | null,
    loadMoreError: null as string | null,
    detailError: null as string | null,
    filters: initialFilters,
    authors: [] as AuthorEntry[],
    selectedHash: null as string | null,
    detail: null as CommitDetail | null,
    detailLoading: false,
    detailCache: new Map<string, CommitDetail>()
}

// Generation + repository epoch for stale-response protection. Every call that
// resets the list (loadFirstPage / setFilters / reset) bumps generation; page,
// authors, and detail responses apply only when both generation and the
// repository root they were issued for still match. Lives in module scope so it
// survives re-renders and store resets within a session.
let generation = 0
let activeRepositoryRoot: string | null = null
// Authors are repository-scoped, not filter/HEAD-scoped. Resolved results stay
// in a process-session cache; in-flight promises are tracked per root so a
// failed request can retry and a late A result cannot overwrite B.
const authorsCache = new Map<string, AuthorEntry[]>()
const authorsInflight = new Map<string, Promise<AuthorEntry[]>>()

function responseMatches(repositoryRoot: string, gen: number): boolean {
    return gen === generation && activeRepositoryRoot === repositoryRoot
}

function requestAuthors(
    repositoryRoot: string,
    publish: (authors: AuthorEntry[]) => void
) {
    if (authorsCache.has(repositoryRoot)) return
    const inflight = authorsInflight.get(repositoryRoot)
    if (inflight) return
    const promise = gitLogAuthors(repositoryRoot)
        .then((authors) => {
            if (authorsInflight.get(repositoryRoot) !== promise) return authors
            authorsCache.set(repositoryRoot, authors)
            authorsInflight.delete(repositoryRoot)
            if (activeRepositoryRoot === repositoryRoot) publish(authors)
            return authors
        })
        .catch((error: unknown) => {
            if (authorsInflight.get(repositoryRoot) === promise) {
                authorsInflight.delete(repositoryRoot)
            }
            throw error
        })
    authorsInflight.set(repositoryRoot, promise)
    void promise.catch(() => {
        // Authors are advisory (filter dropdown); ignore load failure.
    })
}

// Insert into the bounded detail cache, evicting the oldest-inserted entry when
// over the cap. Returns a new Map so zustand sees a fresh reference.
function detailCacheKey(repositoryRoot: string, hash: string): string {
    return `${repositoryRoot}\0${hash}`
}

function cacheDetail(
    cache: Map<string, CommitDetail>,
    repositoryRoot: string,
    hash: string,
    detail: CommitDetail
): Map<string, CommitDetail> {
    const next = new Map(cache)
    next.set(detailCacheKey(repositoryRoot, hash), detail)
    if (next.size > DETAIL_CACHE_LIMIT) {
        const oldest = next.keys().next().value
        if (oldest !== undefined) next.delete(oldest)
    }
    return next
}

export const useGitLogStore = create<GitLogState>()((set, get) => ({
    ...initialGitLogState,

    loadFirstPage: async (repositoryRoot) => {
        const gen = ++generation
        activeRepositoryRoot = repositoryRoot
        const { filters } = get()
        // Own the pagination flag for this epoch immediately. Any in-flight
        // loadMore from a prior root/generation must not later clear B's live
        // loadingMore, so superseding loads clear the flag here and mismatched
        // loadMore responses write nothing.
        const cachedAuthors = authorsCache.get(repositoryRoot)
        set({
            loading: true,
            loadingMore: false,
            listError: null,
            loadMoreError: null,
            nextCursor: null,
            // Publish this root's cached authors immediately, or clear so the
            // previous root's list cannot flash under the new identity.
            authors: cachedAuthors ?? [],
            // New list identity owns detail: settle any in-flight selection.
            selectedHash: null,
            detail: null,
            detailLoading: false,
            detailError: null
        })
        requestAuthors(repositoryRoot, (authors) => set({ authors }))
        try {
            const page = await gitLogPage(
                repositoryRoot,
                null,
                LOG_PAGE_SIZE,
                filters.query || null,
                filters.author,
                filters.since,
                filters.until
            )
            // Stale branch must NOT touch the flag: a gen/root mismatch here
            // means the flag's current owner is a newer reload (which set it
            // true and will clear it itself) or reset() (which already cleared
            // it). Clearing here would open the loadMore gate early with a
            // stale cursor while the newer reload is still in flight.
            if (!responseMatches(repositoryRoot, gen)) return
            set({
                commits: page.commits,
                hasMore: page.hasMore,
                nextCursor: page.nextCursor,
                loading: false,
                listError: null
            })
        } catch (e) {
            if (!responseMatches(repositoryRoot, gen)) return
            set({ listError: String(e), loading: false })
        }
    },

    loadMore: async (repositoryRoot) => {
        const { hasMore, loading, loadingMore, nextCursor, filters } = get()
        if (!hasMore || loading || loadingMore || !nextCursor) return
        if (activeRepositoryRoot !== repositoryRoot) return
        const gen = generation
        set({ loadingMore: true, loadMoreError: null })
        try {
            const page = await gitLogPage(
                repositoryRoot,
                nextCursor,
                LOG_PAGE_SIZE,
                filters.query || null,
                filters.author,
                filters.since,
                filters.until
            )
            // Root/generation mismatch: ZERO state writes. loadFirstPage/reset
            // already own loadingMore for the live epoch; clearing it here would
            // drop a concurrent B loadMore flag when a stale A response lands.
            if (!responseMatches(repositoryRoot, gen)) return
            set((s) => {
                // Cursor pagination is tip-stable, but still dedup by hash so a
                // defensive overlap never produces duplicate React keys.
                const seen = new Set(s.commits.map((c) => c.hash))
                return {
                    commits: [
                        ...s.commits,
                        ...page.commits.filter((c) => !seen.has(c.hash))
                    ],
                    hasMore: page.hasMore,
                    nextCursor: page.nextCursor,
                    loadingMore: false,
                    loadMoreError: null
                }
            })
        } catch (e) {
            if (!responseMatches(repositoryRoot, gen)) return
            // Preserve loaded commits; only surface a load-more-specific error.
            set({ loadMoreError: String(e), loadingMore: false })
        }
    },

    setFilters: (repositoryRoot, partial) => {
        set((s) => ({ filters: { ...s.filters, ...partial } }))
        // Reload the first page under a fresh generation. Debounce (for `query`)
        // is the UI layer's responsibility — the store reloads immediately.
        void get().loadFirstPage(repositoryRoot)
    },

    select: async (repositoryRoot, hash) => {
        if (activeRepositoryRoot !== repositoryRoot) {
            // First selection after mount may precede loadFirstPage establishing
            // the epoch (rare); bind to the requested root so later responses
            // still match.
            activeRepositoryRoot = repositoryRoot
        }
        const gen = generation
        set({ selectedHash: hash, detailError: null })
        const cached = get().detailCache.get(detailCacheKey(repositoryRoot, hash))
        if (cached) {
            set({ detail: cached, detailLoading: false, detailError: null })
            return
        }
        // Cache miss: clear the previous commit's detail before loading. Without
        // this, selecting B while A's detail is still set opens a stale-flash
        // window where detail=A but selectedHash=B — clicking a file row or
        // Compare in that window would open the modal with hash=B / files=A.
        set({ detail: null, detailLoading: true, detailError: null })
        try {
            const detail = await gitCommitDetail(repositoryRoot, hash)
            // Ignore if the selection, list identity, or repository moved on.
            if (get().selectedHash !== hash || !responseMatches(repositoryRoot, gen)) return
            set((s) => ({
                detail,
                detailLoading: false,
                detailError: null,
                detailCache: cacheDetail(s.detailCache, repositoryRoot, hash, detail)
            }))
        } catch (e) {
            if (get().selectedHash !== hash || !responseMatches(repositoryRoot, gen)) return
            // Keep the list intact; only the detail pane shows this error.
            set({ detailError: String(e), detailLoading: false })
        }
    },

    reset: (options) => {
        // Bump the generation and clear the repository epoch so any in-flight
        // page/detail response is dropped, then restore initial displayed state.
        // Full reset (default) also drops the process-session author cache so
        // tests and true teardown stay isolated. LogTab repo switches pass
        // preserveAuthorSession to keep A→B→A from rescanning.
        generation++
        activeRepositoryRoot = null
        if (!options?.preserveAuthorSession) {
            authorsCache.clear()
            authorsInflight.clear()
        }
        set({
            ...initialGitLogState,
            filters: { ...initialFilters },
            detailCache: new Map<string, CommitDetail>()
        })
    }
}))

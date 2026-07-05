import { create } from "zustand"

import { gitCommitDetail, gitLogAuthors, gitLogPage } from "../lib/ipc"
import type { AuthorEntry, CommitDetail, LogCommit } from "../lib/types"

// Page size for the commit list. One request loads this many commits; loadMore
// appends another page. 200 matches the design's "load a healthy window, page on
// scroll" intent without overwhelming the graph layout.
export const LOG_PAGE_SIZE = 200

// Commit-detail cache cap. A simple bounded map (not a strict LRU): once full we
// drop the oldest-inserted entry. Detail payloads are cheap and re-fetchable, so
// eviction accuracy doesn't matter — only unbounded growth does.
export const DETAIL_CACHE_LIMIT = 50

export interface LogFilters {
    query: string
    author: string | null
    since: string | null
    until: string | null
}

const initialFilters: LogFilters = { query: "", author: null, since: null, until: null }

interface GitLogState {
    commits: LogCommit[]
    hasMore: boolean
    loading: boolean // first page load
    loadingMore: boolean // pagination load
    error: string | null
    filters: LogFilters
    authors: AuthorEntry[]
    selectedHash: string | null
    detail: CommitDetail | null
    detailLoading: boolean
    detailCache: Map<string, CommitDetail>

    loadFirstPage: () => Promise<void>
    loadMore: () => Promise<void>
    setFilters: (partial: Partial<LogFilters>) => void
    select: (hash: string) => Promise<void>
    reset: () => void
}

export const initialGitLogState = {
    commits: [] as LogCommit[],
    hasMore: false,
    loading: false,
    loadingMore: false,
    error: null as string | null,
    filters: initialFilters,
    authors: [] as AuthorEntry[],
    selectedHash: null as string | null,
    detail: null as CommitDetail | null,
    detailLoading: false,
    detailCache: new Map<string, CommitDetail>()
}

// Generation counter for stale-response protection. Every call that resets the
// list (loadFirstPage / setFilters) bumps this; a page/detail response is only
// applied if the generation it was issued under still matches. Lives in module
// scope so it survives re-renders and store resets within a session.
let generation = 0

// Insert into the bounded detail cache, evicting the oldest-inserted entry when
// over the cap. Returns a new Map so zustand sees a fresh reference.
function cacheDetail(
    cache: Map<string, CommitDetail>,
    hash: string,
    detail: CommitDetail
): Map<string, CommitDetail> {
    const next = new Map(cache)
    next.set(hash, detail)
    if (next.size > DETAIL_CACHE_LIMIT) {
        const oldest = next.keys().next().value
        if (oldest !== undefined) next.delete(oldest)
    }
    return next
}

export const useGitLogStore = create<GitLogState>()((set, get) => ({
    ...initialGitLogState,

    loadFirstPage: async () => {
        const gen = ++generation
        const { filters } = get()
        set({ loading: true, error: null })
        // Kick off authors load alongside the first page (fire-and-forget; a
        // failure here must not block or error the commit list).
        gitLogAuthors()
            .then((authors) => {
                if (gen === generation) set({ authors })
            })
            .catch(() => {
                // Authors are advisory (filter dropdown); ignore load failure.
            })
        try {
            const page = await gitLogPage(
                0,
                LOG_PAGE_SIZE,
                filters.query || null,
                filters.author,
                filters.since,
                filters.until
            )
            // Stale branch must NOT touch the flag: a gen mismatch here means
            // the flag's current owner is a newer reload (which set it true and
            // will clear it itself) or reset() (which already cleared it).
            // Clearing here would open the loadMore gate early with a stale
            // skip while the newer reload is still in flight.
            if (gen !== generation) return
            set({ commits: page.commits, hasMore: page.hasMore, loading: false })
        } catch (e) {
            if (gen !== generation) return
            set({ error: String(e), loading: false })
        }
    },

    loadMore: async () => {
        const { hasMore, loading, loadingMore, commits, filters } = get()
        if (!hasMore || loading || loadingMore) return
        const gen = generation
        set({ loadingMore: true })
        try {
            const page = await gitLogPage(
                commits.length,
                LOG_PAGE_SIZE,
                filters.query || null,
                filters.author,
                filters.since,
                filters.until
            )
            if (gen !== generation) {
                // Filters changed mid-flight: drop the stale page, but ALWAYS
                // release the flag — the reload triggered by setFilters only
                // manages `loading`, so leaving `loadingMore` true here would
                // permanently gate-block every future loadMore.
                set({ loadingMore: false })
                return
            }
            set((s) => ({
                commits: [...s.commits, ...page.commits],
                hasMore: page.hasMore,
                loadingMore: false
            }))
        } catch (e) {
            if (gen !== generation) {
                set({ loadingMore: false })
                return
            }
            set({ error: String(e), loadingMore: false })
        }
    },

    setFilters: (partial) => {
        set((s) => ({ filters: { ...s.filters, ...partial } }))
        // Reload the first page under a fresh generation. Debounce (for `query`)
        // is the UI layer's responsibility — the store reloads immediately.
        void get().loadFirstPage()
    },

    select: async (hash) => {
        set({ selectedHash: hash })
        const cached = get().detailCache.get(hash)
        if (cached) {
            set({ detail: cached, detailLoading: false })
            return
        }
        // Cache miss: clear the previous commit's detail before loading. Without
        // this, selecting B while A's detail is still set opens a stale-flash
        // window where detail=A but selectedHash=B — clicking a file row or
        // Compare in that window would open the modal with hash=B / files=A.
        set({ detail: null, detailLoading: true })
        try {
            const detail = await gitCommitDetail(hash)
            // Ignore if the selection moved on while this was in flight.
            if (get().selectedHash !== hash) return
            set((s) => ({
                detail,
                detailLoading: false,
                detailCache: cacheDetail(s.detailCache, hash, detail)
            }))
        } catch (e) {
            if (get().selectedHash !== hash) return
            set({ error: String(e), detailLoading: false })
        }
    },

    reset: () => {
        // Bump the generation so any in-flight page/authors response is dropped,
        // then restore initial state with fresh mutable containers.
        generation++
        set({
            ...initialGitLogState,
            filters: { ...initialFilters },
            detailCache: new Map<string, CommitDetail>()
        })
    }
}))

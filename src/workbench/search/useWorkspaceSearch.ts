import { useEffect, useRef, useState } from "react"

import { searchWorkspace } from "@/lib/ipc"
import type { SearchEvent } from "@/lib/types"
import { useWorkspaceStore } from "@/state/workspaceStore"

interface UseWorkspaceSearch {
    events: SearchEvent[]
    loading: boolean
}

/**
 * Workspace full-text search as a hook — the query/debounce/cancellation logic
 * lifted verbatim from FilesNavContent so it can be reused by the command
 * palette. A trimmed query of at least 2 chars streams results after a 250ms
 * debounce (the 2-char floor keeps a single keystroke from scanning the whole
 * tree — mirrored in CommandPalette); stale responses are dropped via a per-run
 * `cancelled` flag, and clearing the query fires an empty search so the Rust
 * generation advances and any still-running query stops (m6, front-end
 * cancellation — no new command). Streamed events are buffered in a ref and
 * flushed to state on a 50ms tick so a fast, high-volume stream costs one
 * setState per tick instead of one per event (O(n) instead of O(n²) renders).
 * `loading` is high from when the debounced search fires until its `done` event
 * (or until the query is cleared/cancelled).
 */
const MIN_QUERY_LEN = 2
const FLUSH_MS = 50

export function useWorkspaceSearch(query: string, caseSensitive: boolean): UseWorkspaceSearch {
    const workspacePath = useWorkspaceStore((state) => state.workspacePath)
    const [events, setEvents] = useState<SearchEvent[]>([])
    const [loading, setLoading] = useState(false)

    const trimmed = query.trim()
    const hadQuery = useRef(false)

    useEffect(() => {
        if (!workspacePath || trimmed.length < MIN_QUERY_LEN) {
            setEvents([])
            setLoading(false)
            // Leaving/clearing search: fire an empty search so the Rust generation
            // advances and any still-running query sees a stale generation and stops
            // (m6, front-end cancellation — no new command). Only when a query was
            // actually active, so first mount doesn't emit a spurious search.
            if (workspacePath && hadQuery.current) {
                void searchWorkspace(workspacePath, "", caseSensitive, () => {})
            }
            hadQuery.current = false
            return
        }
        hadQuery.current = true
        // Clear old results immediately so a previous query's hits don't linger
        // through the debounce when switching queries (T18).
        setEvents([])
        setLoading(true)
        let cancelled = false
        const buffer: SearchEvent[] = []
        let flushTimer: ReturnType<typeof setInterval> | null = null
        const flush = () => {
            if (buffer.length === 0) return
            const batch = buffer.splice(0, buffer.length)
            setEvents((prev) => [...prev, ...batch])
        }
        const timer = setTimeout(() => {
            flushTimer = setInterval(flush, FLUSH_MS)
            void searchWorkspace(workspacePath, trimmed, caseSensitive, (e) => {
                if (cancelled) return
                buffer.push(e)
                if (e.type === "done") {
                    if (flushTimer) {
                        clearInterval(flushTimer)
                        flushTimer = null
                    }
                    flush()
                    setLoading(false)
                }
            })
        }, 250)
        return () => {
            cancelled = true
            clearTimeout(timer)
            if (flushTimer) clearInterval(flushTimer)
        }
    }, [workspacePath, trimmed, caseSensitive])

    return { events, loading }
}

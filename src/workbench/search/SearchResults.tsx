import { SearchX } from "lucide-react"

import { EmptyState } from "@/app/workbench/EmptyState"
import type { SearchEvent, SearchMatch } from "@/lib/types"
import { useWorkspaceStore } from "@/state/workspaceStore"

interface FileGroup {
    path: string
    matches: SearchMatch[]
}

function groupByFile(events: SearchEvent[]): FileGroup[] {
    const order: string[] = []
    const byPath = new Map<string, SearchMatch[]>()
    for (const e of events) {
        if (e.type !== "match") continue
        let bucket = byPath.get(e.path)
        if (!bucket) {
            bucket = []
            byPath.set(e.path, bucket)
            order.push(e.path)
        }
        bucket.push(...e.matches)
    }
    return order.map((path) => ({ path, matches: byPath.get(path)! }))
}

function doneEvent(events: SearchEvent[]) {
    return events.find((e) => e.type === "done") as
        | Extract<SearchEvent, { type: "done" }>
        | undefined
}

// Split preview around the query so the matched substring renders as its own
// <mark>. Case-insensitive to mirror the default (case-insensitive) search.
function highlight(preview: string, query: string) {
    if (!query) return [preview]
    const lower = preview.toLowerCase()
    const needle = query.toLowerCase()
    const parts: (string | { mark: string })[] = []
    let from = 0
    let at = lower.indexOf(needle, from)
    while (at !== -1) {
        if (at > from) parts.push(preview.slice(from, at))
        parts.push({ mark: preview.slice(at, at + query.length) })
        from = at + query.length
        at = lower.indexOf(needle, from)
    }
    if (from < preview.length) parts.push(preview.slice(from))
    return parts
}

function relativePath(path: string, root: string | null) {
    if (root && path.startsWith(root + "/")) return path.slice(root.length + 1)
    return path
}

function baseName(path: string) {
    return path.split("/").pop() ?? path
}

export function SearchResults({
    events,
    query
}: {
    events: SearchEvent[]
    query: string
}) {
    const workspacePath = useWorkspaceStore((s) => s.workspacePath)
    const requestReveal = useWorkspaceStore((s) => s.requestReveal)
    const groups = groupByFile(events)
    const done = doneEvent(events)

    const truncationNotice = done?.truncated ? (
        <div className="mx-[6px] mb-[4px] rounded-[8px] bg-(--yz-hover) px-[10px] py-[6px] text-[12px] text-(--ink-2)">
            已達 5,000 檔上限，請縮小範圍
        </div>
    ) : null

    if (done && groups.length === 0) {
        return (
            <div className="flex h-full flex-col">
                {truncationNotice}
                <div className="flex flex-1 items-center justify-center">
                    <EmptyState
                        icon={SearchX}
                        title="沒有符合的結果"
                        description="換個關鍵字或關閉大小寫比對再試一次"
                    />
                </div>
            </div>
        )
    }

    return (
        <div className="flex flex-col gap-[2px] py-[4px]">
            {truncationNotice}
            {groups.map((group) => (
                <div key={group.path} className="flex flex-col">
                    <div className="flex items-baseline gap-[7px] px-[14px] pt-[6px] pb-[2px]">
                        <span className="truncate text-[12.5px] font-semibold text-(--ink-1)">
                            {baseName(group.path)}
                        </span>
                        <span className="truncate text-[11px] text-(--ink-3)">
                            {relativePath(group.path, workspacePath)}
                        </span>
                    </div>
                    <ul className="flex flex-col gap-[1px]">
                        {group.matches.map((m, i) => (
                            <li key={`${m.line}:${m.col}:${i}`}>
                                <button
                                    type="button"
                                    onClick={() => requestReveal(group.path, m.line, false)}
                                    className="flex h-[24px] w-full items-center gap-[10px] rounded-[8px] pr-[8px] pl-[26px] text-left text-[12.5px] transition-colors duration-100 hover:bg-(--yz-hover)"
                                >
                                    <span className="w-[34px] shrink-0 text-right font-mono text-[11px] text-(--ink-3) tabular-nums">
                                        {m.line}
                                    </span>
                                    <span className="truncate text-(--ink-2)">
                                        {highlight(m.preview, query).map((part, j) =>
                                            typeof part === "string" ? (
                                                <span key={j}>{part}</span>
                                            ) : (
                                                <mark
                                                    key={j}
                                                    className="rounded-[2px] bg-(--yz-active) px-[1px] text-(--ink-0)"
                                                >
                                                    {part.mark}
                                                </mark>
                                            )
                                        )}
                                    </span>
                                </button>
                            </li>
                        ))}
                    </ul>
                </div>
            ))}
        </div>
    )
}

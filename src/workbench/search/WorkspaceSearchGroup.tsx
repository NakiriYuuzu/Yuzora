import { Loader2 } from "lucide-react"
import { useTranslation } from "react-i18next"

import { CommandGroup, CommandItem } from "@/components/ui/command"
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

/**
 * "工作區搜尋" section for the command palette — the streaming full-text search
 * results rendered as cmdk items so keyboard navigation and Enter-to-open work
 * alongside the command list. Adapted from the old standalone SearchResults:
 * a section header carrying the case-sensitivity toggle and a loading spinner,
 * then one cmdk group per file (filename + relative path heading) whose rows
 * show the line number and a highlighted excerpt. Selecting a row reveals it.
 */
export function WorkspaceSearchGroup({
    events,
    query,
    loading,
    caseSensitive,
    onToggleCaseSensitive,
    onReveal,
}: {
    events: SearchEvent[]
    query: string
    loading: boolean
    caseSensitive: boolean
    onToggleCaseSensitive: () => void
    onReveal: (path: string, line: number) => void
}) {
    const { t } = useTranslation("menus")
    const workspacePath = useWorkspaceStore((s) => s.workspacePath)
    const groups = groupByFile(events)
    const done = doneEvent(events)

    return (
        <>
            <div className="flex items-center gap-[8px] px-[14px] pt-[8px] pb-[4px]">
                <span className="text-[10px] font-semibold tracking-wider text-(--ink-3) uppercase">
                    {t("workspaceSearch.heading")}
                </span>
                {loading && (
                    <Loader2 className="size-[12px] animate-spin text-(--ink-3)" aria-hidden="true" />
                )}
                <div className="flex-1" />
                <button
                    type="button"
                    aria-label={t("workspaceSearch.matchCaseAriaLabel")}
                    aria-pressed={caseSensitive}
                    // preventDefault keeps focus on the palette input so typing continues.
                    onMouseDown={(e) => e.preventDefault()}
                    onClick={onToggleCaseSensitive}
                    className={`rounded-[5px] px-[6px] py-[2px] font-mono text-[11px] transition-colors ${
                        caseSensitive ? "bg-(--yz-active) text-(--ink-0)" : "text-(--ink-3) hover:bg-(--yz-hover)"
                    }`}
                >
                    Aa
                </button>
            </div>

            {done?.truncated && (
                <div className="mx-[8px] mb-[4px] rounded-[8px] bg-(--yz-hover) px-[10px] py-[6px] text-[12px] text-(--ink-2)">
                    {t("workspaceSearch.truncatedNotice")}
                </div>
            )}

            {groups.map((group) => (
                <CommandGroup
                    key={group.path}
                    value={group.path}
                    heading={
                        <span className="flex items-baseline gap-[7px]">
                            <span className="truncate text-[12.5px] font-semibold text-(--ink-1)">
                                {baseName(group.path)}
                            </span>
                            <span className="truncate text-[11px] text-(--ink-3)">
                                {relativePath(group.path, workspacePath)}
                            </span>
                        </span>
                    }
                    className="p-0 pb-[2px]"
                >
                    {group.matches.map((m, i) => (
                        <CommandItem
                            key={`${m.line}:${m.col}:${i}`}
                            value={`ws:${group.path}:${m.line}:${m.col}:${i}`}
                            onSelect={() => onReveal(group.path, m.line)}
                            className="h-[26px] gap-[10px] rounded-[8px]! px-[8px] pl-[26px] text-[12.5px] data-selected:bg-(--yz-active)"
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
                        </CommandItem>
                    ))}
                </CommandGroup>
            ))}

            {done && groups.length === 0 && (
                <div className="px-[14px] py-[10px] text-[12.5px] text-(--ink-3)">
                    {t("workspaceSearch.noResults")}
                </div>
            )}
        </>
    )
}

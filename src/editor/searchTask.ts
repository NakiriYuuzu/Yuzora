import { EditorState, type Text } from "@codemirror/state"
import { SearchQuery } from "@codemirror/search"

export type SearchAction = "next" | "prev" | "select" | "replace" | "replaceAll"
export interface SearchTask {
    id: number
    lines?: string[]
    action: SearchAction
    from: number
    to: number
    wordChars?: string
    query: { search: string; replace: string; caseSensitive: boolean; regexp: boolean; wholeWord: boolean; literal: boolean }
}
export interface SearchTaskResult {
    id: number
    ranges: Array<{ from: number; to: number }>
    changes?: Array<{ from: number; to: number; insert: string }>
    error?: "tooMany" | "failed"
}

/** Use the same Unicode, regex and whole-word semantics as CodeMirror, off the UI thread. */
export function runSearchTask(doc: Text, task: SearchTask): SearchTaskResult {
    const state = EditorState.create({ doc, selection: { anchor: task.from, head: task.to }, extensions: task.wordChars ? EditorState.languageData.of(() => [{ wordChars: task.wordChars }]) : [] })
    const query = new SearchQuery(task.query)
    const result: SearchTaskResult = { id: task.id, ranges: [] }
    if (!query.valid) return result
    // getCursor's public return type erases the precise/regex match fields that
    // both concrete CodeMirror cursors provide.
    type Match = { from: number; to: number; precise: boolean; match?: RegExpExecArray }
    const cursor = (from: number, to = doc.length) => query.getCursor(state, from, to) as Iterator<Match> & { nextOverlapping?: () => IteratorResult<Match> }
    const queryLength = (query.literal ? query.search : query.search.replace(/\\([nrt\\])/g, "_")).length
    const next = (from: number, wrap: number) => {
        let found = cursor(from).next()
        if (found.done) found = cursor(0, query.regexp ? wrap : Math.min(doc.length, wrap + queryLength)).next()
        return found.done ? null : found.value
    }
    if (task.action === "next" || task.action === "replace") {
        const match = next(task.action === "replace" ? task.from : task.to, task.to)
        if (match) {
            if (task.action === "replace" && match.precise && match.from === task.from && match.to === task.to) {
                result.changes = [{ from: match.from, to: match.to, insert: replacement(query, match) }]
                const following = next(match.to, match.from)
                if (following) result.ranges = [{ from: following.from, to: following.to }]
            } else if (task.action === "replace" && !match.precise) {
                const following = next(match.to, match.from)
                if (following) result.ranges = [{ from: following.from, to: following.to }]
            } else result.ranges = [{ from: match.from, to: match.to }]
        }
    } else if (task.action === "prev") {
        // Chunking belongs to CodeMirror's cursor. Running in a worker also keeps
        // multiline regexes and scans with no matches away from the event loop.
        const last = (from: number, to: number) => {
            let match: { from: number; to: number } | null = null
            const iter = cursor(from, to)
            const advance = () => iter.nextOverlapping ? iter.nextOverlapping() : iter.next()
            for (let found = advance(); !found.done; found = advance()) match = { from: found.value.from, to: found.value.to }
            return match
        }
        const match = last(0, task.from) ?? last(query.regexp ? task.to : Math.max(0, task.from - queryLength), doc.length)
        if (match) result.ranges = [match]
    } else {
        const limit = task.action === "select" ? 1000 : 100000
        const changes: NonNullable<SearchTaskResult["changes"]> = []
        const iter = cursor(0)
        for (let found = iter.next(); !found.done; found = iter.next()) {
            const match = found.value
            if (result.ranges.length >= limit) return { id: task.id, ranges: [], error: "tooMany" }
            result.ranges.push({ from: match.from, to: match.to })
            if (task.action === "replaceAll" && match.precise) changes.push({ from: match.from, to: match.to, insert: replacement(query, match) })
        }
        if (task.action === "replaceAll") { result.changes = changes; result.ranges = [] }
    }
    return result
}

function replacement(query: SearchQuery, match: { match?: RegExpExecArray }): string {
    const text = query.literal ? query.replace : query.replace.replace(/\\([nrt\\])/g, (_, c: string) => ({ n: "\n", r: "\r", t: "\t", "\\": "\\" })[c]!)
    if (!query.regexp || !match.match) return text
    const groups = match.match
    return text.replace(/\$([$&]|\d+)/g, (token, index: string) => {
        if (index === "$") return "$"
        if (index === "&") return groups[0]
        for (let size = index.length; size > 0; size--) {
            const n = Number(index.slice(0, size))
            if (n > 0 && n < groups.length) return (groups[n] ?? "") + index.slice(size)
        }
        return token
    })
}

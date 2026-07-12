import type { DbError, DbKind, DbObjectReference } from "./types"

export type DatabaseSqlDialect = "sqlite" | "postgres" | "mssql"

export function dbObjectRefKey(object: DbObjectReference): string {
    return JSON.stringify([object.catalog, object.schema, object.kind, object.name])
}

function quoteDoubleIdentifier(segment: string): string {
    return `"${segment.replace(/"/g, '""')}"`
}

function quoteMssqlIdentifier(segment: string): string {
    const escaped = segment.split("]").join("]]")
    return `[${escaped}]`
}

/** A qualified, bounded preview query for one exact object reference. PostgreSQL
 * catalog is identity/filter metadata only because one connection cannot query
 * another database; SQLite's catalog/schema both name the same namespace. */
export function buildTableQuery(kind: DbKind, object: DbObjectReference): string {
    if (kind === "mssql") {
        const qualified = [object.catalog, object.schema, object.name]
            .map(quoteMssqlIdentifier)
            .join(".")
        return `SELECT TOP 100 * FROM ${qualified}`
    }
    const qualified = [object.schema, object.name].map(quoteDoubleIdentifier).join(".")
    return `SELECT * FROM ${qualified} LIMIT 100`
}

export interface DatabaseErrorSelection {
    from: number
    to: number
}

function selectCodePointAt(source: string, from: number): DatabaseErrorSelection {
    const codePoint = source.codePointAt(from)
    return {
        from,
        to: codePoint === undefined ? from : from + (codePoint > 0xffff ? 2 : 1)
    }
}

function utf16OffsetForCharacterIndex(source: string, characterIndex: number): number | null {
    if (!Number.isSafeInteger(characterIndex) || characterIndex < 0) return null
    let characters = 0
    let utf16Offset = 0
    for (const character of source) {
        if (characters === characterIndex) return utf16Offset
        characters += 1
        utf16Offset += character.length
    }
    return characters === characterIndex ? utf16Offset : null
}

function utf16OffsetForUtf8Byte(source: string, byteOffset: number): number | null {
    if (!Number.isSafeInteger(byteOffset) || byteOffset < 0) return null
    const encoder = new TextEncoder()
    let bytes = 0
    let utf16Offset = 0
    for (const character of source) {
        if (bytes === byteOffset) return utf16Offset
        const nextBytes = bytes + encoder.encode(character).length
        if (byteOffset < nextBytes) return null
        bytes = nextBytes
        utf16Offset += character.length
    }
    return bytes === byteOffset ? utf16Offset : null
}

function utf16OffsetForLine(source: string, oneBasedLine: number): number | null {
    if (!Number.isSafeInteger(oneBasedLine) || oneBasedLine < 1) return null
    let offset = 0
    for (let line = 1; line < oneBasedLine; line += 1) {
        const newline = source.indexOf("\n", offset)
        if (newline < 0) return null
        offset = newline + 1
    }
    return offset
}

/** Map engine-native locations to CodeMirror/JavaScript UTF-16 offsets.
 * PostgreSQL offsets are 1-based Unicode characters, SQLite offsets are
 * 0-based UTF-8 bytes, and MSSQL locations identify a 1-based line start. */
export function databaseErrorSelection(
    executedSql: string,
    error: DbError
): DatabaseErrorSelection | null {
    if (!error.position) return null
    if (error.engine === "postgres" && error.position.offset !== null) {
        const from = utf16OffsetForCharacterIndex(executedSql, error.position.offset - 1)
        return from === null ? null : selectCodePointAt(executedSql, from)
    }
    if (error.engine === "sqlite" && error.position.offset !== null) {
        const from = utf16OffsetForUtf8Byte(executedSql, error.position.offset)
        return from === null ? null : selectCodePointAt(executedSql, from)
    }
    if (error.engine === "mssql" && error.position.line !== null) {
        const from = utf16OffsetForLine(executedSql, error.position.line)
        return from === null ? null : selectCodePointAt(executedSql, from)
    }
    return null
}

export function databaseErrorSelectionForEditor(
    editorSql: string,
    executedSql: string,
    error: DbError
): DatabaseErrorSelection | null {
    return editorSql === executedSql ? databaseErrorSelection(executedSql, error) : null
}

/** UTF-16 offsets into the original editor buffer (the same offsets used by JS slice/CodeMirror). */
export type DatabaseSqlTransactionBoundary = "none" | "begin" | "commit" | "rollback"

export interface DatabaseSqlUnit {
    sql: string
    start: number
    end: number
    transactionBoundary: DatabaseSqlTransactionBoundary
}

export type DatabaseSqlParseErrorCode =
    | "unterminated-string"
    | "unterminated-identifier"
    | "unterminated-comment"
    | "unterminated-dollar-quote"
    | "ambiguous-string-escape"
    | "ambiguous-batch-separator"
    | "unbalanced-block"
    | "unexpected-end"
    | "unbalanced-parenthesis"
    | "unexpected-closing-parenthesis"

export interface DatabaseSqlParseError {
    code: DatabaseSqlParseErrorCode
    message: string
    position: number
}

export type DatabaseSqlSplitResult =
    | { ok: true; units: DatabaseSqlUnit[] }
    | { ok: false; error: DatabaseSqlParseError }

type BlockKind = "begin" | "case"

interface BlockFrame {
    kind: BlockKind
    position: number
}

interface GoLine {
    kind: "separator" | "ambiguous"
    next: number
}

function isWhitespace(char: string): boolean {
    return /\s/.test(char)
}

function isWordStart(char: string | undefined): boolean {
    return char !== undefined && /[A-Za-z_#@]/.test(char)
}

function isWordPart(char: string | undefined): boolean {
    return char !== undefined && /[A-Za-z0-9_@$#]/.test(char)
}

function fail(
    code: DatabaseSqlParseErrorCode,
    message: string,
    position: number
): DatabaseSqlSplitResult {
    return { ok: false, error: { code, message, position } }
}

function dollarDelimiterAt(source: string, index: number): string | null {
    if (source[index] !== "$" || isWordPart(source[index - 1])) return null
    if (source[index + 1] === "$") return "$$"
    if (!/[A-Za-z_]/.test(source[index + 1] ?? "")) return null

    let end = index + 2
    while (/[A-Za-z0-9_]/.test(source[end] ?? "")) end += 1
    return source[end] === "$" ? source.slice(index, end + 1) : null
}

function usesPostgresEscapePrefix(source: string, quote: number): boolean {
    const previous = source[quote - 1]
    if ((previous === "e" || previous === "E") && !isWordPart(source[quote - 2])) return true
    return (
        source[quote - 1] === "&" &&
        (source[quote - 2] === "u" || source[quote - 2] === "U") &&
        !isWordPart(source[quote - 3])
    )
}

function nextKeyword(source: string, from: number): string | null {
    let index = from
    while (index < source.length) {
        if (isWhitespace(source[index])) {
            index += 1
            continue
        }
        if (source.startsWith("--", index)) {
            const newline = source.indexOf("\n", index + 2)
            if (newline < 0) return null
            index = newline + 1
            continue
        }
        if (source.startsWith("/*", index)) {
            const close = source.indexOf("*/", index + 2)
            if (close < 0) return null
            index = close + 2
            continue
        }
        if (!isWordStart(source[index])) return null
        let end = index + 1
        while (isWordPart(source[end])) end += 1
        return source.slice(index, end).toUpperCase()
    }
    return null
}

function inspectGoLine(source: string, index: number, lineStart: number): GoLine | null {
    if (source.slice(index, index + 2).toUpperCase() !== "GO") return null
    if (isWordPart(source[index + 2])) return null
    if (!/^[\t \f\v\r]*$/.test(source.slice(lineStart, index))) return null

    const newline = source.indexOf("\n", index + 2)
    const lineEnd = newline < 0 ? source.length : newline
    const suffix = source.slice(index + 2, lineEnd).replace(/\r$/, "")
    const next = newline < 0 ? source.length : newline + 1
    return /^[\t ]*(?:--[^\r\n]*)?$/.test(suffix)
        ? { kind: "separator", next }
        : { kind: "ambiguous", next }
}

function isMssqlRoutine(words: string[]): boolean {
    const routineKinds = new Set(["PROC", "PROCEDURE", "FUNCTION", "TRIGGER"])
    return (
        ((words[0] === "CREATE" || words[0] === "ALTER") && routineKinds.has(words[1])) ||
        (words[0] === "CREATE" &&
            words[1] === "OR" &&
            words[2] === "ALTER" &&
            routineKinds.has(words[3]))
    )
}

function isSqliteTrigger(words: string[]): boolean {
    return (
        (words[0] === "CREATE" && words[1] === "TRIGGER") ||
        (words[0] === "CREATE" &&
            (words[1] === "TEMP" || words[1] === "TEMPORARY") &&
            words[2] === "TRIGGER")
    )
}

function transactionBoundaryFor(
    dialect: DatabaseSqlDialect,
    words: string[]
): DatabaseSqlTransactionBoundary {
    if (words[0] === "COMMIT") return "commit"
    if (words[0] === "ROLLBACK") return "rollback"
    if (words[0] === "START" && words[1] === "TRANSACTION") return "begin"
    if (words[0] !== "BEGIN") return "none"
    if (dialect !== "mssql") return "begin"
    return words[1] === "TRAN" || words[1] === "TRANSACTION" || words[1] === "DISTRIBUTED"
        ? "begin"
        : "none"
}

/**
 * Split a SQL editor buffer without normalising or rewriting it.
 *
 * A failed parse never exposes partial units, so callers can reject the whole run
 * before dispatching any SQL. Unit ranges are half-open and `unit.sql` is always
 * exactly `source.slice(unit.start, unit.end)`.
 */
export function splitDatabaseSql(
    source: string,
    dialect: DatabaseSqlDialect
): DatabaseSqlSplitResult {
    const units: DatabaseSqlUnit[] = []
    let firstSource: number | null = null
    let hasSql = false
    let lineStart = 0
    let leadingWords: string[] = []
    let mssqlRoutine = false
    let sqliteTrigger = false
    let sqliteTriggerBodySeen = false
    let sqliteTriggerClosed = false
    let blocks: BlockFrame[] = []
    let parentheses: number[] = []

    const mark = (position: number, sql: boolean) => {
        if (firstSource === null) firstSource = position
        if (sql) hasSql = true
    }

    const resetUnitState = () => {
        firstSource = null
        hasSql = false
        leadingWords = []
        mssqlRoutine = false
        sqliteTrigger = false
        sqliteTriggerBodySeen = false
        sqliteTriggerClosed = false
        blocks = []
        parentheses = []
    }

    const emit = (end: number) => {
        if (hasSql && firstSource !== null) {
            let exactEnd = end
            while (exactEnd > firstSource && isWhitespace(source[exactEnd - 1])) exactEnd -= 1
            if (exactEnd > firstSource) {
                units.push({
                    sql: source.slice(firstSource, exactEnd),
                    start: firstSource,
                    end: exactEnd,
                    transactionBoundary: transactionBoundaryFor(dialect, leadingWords)
                })
            }
        }
        resetUnitState()
    }

    const recordWord = (word: string, position: number): DatabaseSqlSplitResult | null => {
        if (leadingWords.length < 4) {
            leadingWords.push(word)
            if (dialect === "mssql" && isMssqlRoutine(leadingWords)) mssqlRoutine = true
            if (dialect === "sqlite" && isSqliteTrigger(leadingWords)) sqliteTrigger = true
        }

        if (dialect === "mssql") {
            if (word === "BEGIN") {
                const following = nextKeyword(source, position + word.length)
                if (
                    following !== "TRAN" &&
                    following !== "TRANSACTION" &&
                    following !== "DISTRIBUTED" &&
                    following !== "DIALOG" &&
                    following !== "CONVERSATION"
                ) {
                    blocks.push({ kind: "begin", position })
                }
            } else if (word === "CASE") {
                blocks.push({ kind: "case", position })
            } else if (word === "END" && nextKeyword(source, position + word.length) !== "CONVERSATION") {
                if (blocks.length === 0) {
                    return fail("unexpected-end", "END has no matching BEGIN or CASE", position)
                }
                blocks.pop()
            }
        }

        if (dialect === "sqlite" && sqliteTrigger) {
            if (word === "BEGIN") {
                sqliteTriggerBodySeen = true
                sqliteTriggerClosed = false
                blocks.push({ kind: "begin", position })
            } else if (word === "CASE") {
                blocks.push({ kind: "case", position })
            } else if (word === "END") {
                if (blocks.length === 0) {
                    return fail("unexpected-end", "END has no matching trigger BEGIN or CASE", position)
                }
                blocks.pop()
                sqliteTriggerClosed = sqliteTriggerBodySeen && blocks.length === 0
            }
        }
        return null
    }

    let index = 0
    while (index < source.length) {
        const char = source[index]

        if (isWhitespace(char)) {
            if (char === "\n" || (char === "\r" && source[index + 1] !== "\n")) {
                lineStart = index + 1
            }
            index += 1
            continue
        }

        if (source.startsWith("--", index)) {
            mark(index, false)
            const newline = source.indexOf("\n", index + 2)
            index = newline < 0 ? source.length : newline
            continue
        }

        if (source.startsWith("/*", index)) {
            const start = index
            mark(start, false)
            let depth = 1
            index += 2
            while (index < source.length && depth > 0) {
                if (dialect !== "sqlite" && source.startsWith("/*", index)) {
                    depth += 1
                    index += 2
                } else if (source.startsWith("*/", index)) {
                    depth -= 1
                    index += 2
                } else {
                    if (source[index] === "\n") lineStart = index + 1
                    index += 1
                }
            }
            if (depth > 0) return fail("unterminated-comment", "Block comment is not closed", start)
            continue
        }

        if (char === "'") {
            const start = index
            const postgresEscapes = dialect === "postgres" && usesPostgresEscapePrefix(source, start)
            mark(start, true)
            index += 1
            let closed = false
            while (index < source.length) {
                if (source[index] === "\\") {
                    if (dialect === "postgres" && !postgresEscapes) {
                        return fail(
                            "ambiguous-string-escape",
                            "PostgreSQL string backslash semantics depend on server configuration; use an E-prefixed string",
                            index
                        )
                    }
                    if (postgresEscapes) {
                        index += 2
                        continue
                    }
                }
                if (source[index] === "'") {
                    if (source[index + 1] === "'") {
                        index += 2
                    } else {
                        index += 1
                        closed = true
                        break
                    }
                } else {
                    if (source[index] === "\n") lineStart = index + 1
                    index += 1
                }
            }
            if (!closed) return fail("unterminated-string", "Quoted string is not closed", start)
            continue
        }

        if (char === '"') {
            const start = index
            const postgresEscapes = dialect === "postgres" && usesPostgresEscapePrefix(source, start)
            mark(start, true)
            index += 1
            let closed = false
            while (index < source.length) {
                if (postgresEscapes && source[index] === "\\") {
                    index += 2
                } else if (source[index] === '"') {
                    if (source[index + 1] === '"') {
                        index += 2
                    } else {
                        index += 1
                        closed = true
                        break
                    }
                } else {
                    if (source[index] === "\n") lineStart = index + 1
                    index += 1
                }
            }
            if (!closed) return fail("unterminated-identifier", "Quoted identifier is not closed", start)
            continue
        }

        if (dialect === "sqlite" && char === "`") {
            const start = index
            mark(start, true)
            index += 1
            let closed = false
            while (index < source.length) {
                if (source[index] === "`") {
                    if (source[index + 1] === "`") index += 2
                    else {
                        index += 1
                        closed = true
                        break
                    }
                } else {
                    if (source[index] === "\n") lineStart = index + 1
                    index += 1
                }
            }
            if (!closed) return fail("unterminated-identifier", "Backtick identifier is not closed", start)
            continue
        }

        if ((dialect === "sqlite" || dialect === "mssql") && char === "[") {
            const start = index
            mark(start, true)
            index += 1
            let closed = false
            while (index < source.length) {
                if (source[index] === "]") {
                    if (dialect === "mssql" && source[index + 1] === "]") index += 2
                    else {
                        index += 1
                        closed = true
                        break
                    }
                } else {
                    if (source[index] === "\n") lineStart = index + 1
                    index += 1
                }
            }
            if (!closed) return fail("unterminated-identifier", "Bracket identifier is not closed", start)
            continue
        }

        if (dialect === "postgres" && char === "$") {
            const delimiter = dollarDelimiterAt(source, index)
            if (delimiter) {
                const start = index
                mark(start, true)
                const close = source.indexOf(delimiter, start + delimiter.length)
                if (close < 0) {
                    return fail(
                        "unterminated-dollar-quote",
                        `Dollar quote ${delimiter} is not closed`,
                        start
                    )
                }
                const body = source.slice(start, close + delimiter.length)
                const lastNewline = body.lastIndexOf("\n")
                if (lastNewline >= 0) lineStart = start + lastNewline + 1
                index = close + delimiter.length
                continue
            }
        }

        if (dialect === "mssql" && (char === "g" || char === "G")) {
            const go = inspectGoLine(source, index, lineStart)
            if (go?.kind === "ambiguous") {
                return fail(
                    "ambiguous-batch-separator",
                    "GO must appear alone on its line; repeat counts and trailing SQL are not supported",
                    index
                )
            }
            if (go?.kind === "separator") {
                if (parentheses.length > 0) {
                    return fail(
                        "unbalanced-parenthesis",
                        "Parenthesis crosses a GO batch boundary",
                        parentheses[parentheses.length - 1]
                    )
                }
                if (blocks.length > 0) {
                    const open = blocks[blocks.length - 1]
                    return fail(
                        "unbalanced-block",
                        `${open.kind.toUpperCase()} block crosses a GO batch boundary`,
                        open.position
                    )
                }
                emit(lineStart)
                index = go.next
                lineStart = go.next
                continue
            }
        }

        if (isWordStart(char)) {
            const start = index
            index += 1
            while (isWordPart(source[index])) index += 1
            mark(start, true)
            const error = recordWord(source.slice(start, index).toUpperCase(), start)
            if (error) return error
            continue
        }

        if (char === "(") {
            mark(index, true)
            parentheses.push(index)
            index += 1
            continue
        }

        if (char === ")") {
            mark(index, true)
            if (parentheses.length === 0) {
                return fail(
                    "unexpected-closing-parenthesis",
                    "Closing parenthesis has no matching opening parenthesis",
                    index
                )
            }
            parentheses.pop()
            index += 1
            continue
        }

        if (char === ";") {
            if (parentheses.length > 0) {
                return fail(
                    "unbalanced-parenthesis",
                    "Opening parenthesis is not closed before the statement boundary",
                    parentheses[parentheses.length - 1]
                )
            }
            if (dialect === "sqlite" && sqliteTrigger) {
                if (sqliteTriggerClosed) emit(index + 1)
            } else if (dialect !== "mssql" || (!mssqlRoutine && blocks.length === 0)) {
                emit(index + 1)
            }
            index += 1
            continue
        }
        mark(index, true)
        index += 1
    }

    if (parentheses.length > 0) {
        return fail(
            "unbalanced-parenthesis",
            "Opening parenthesis is not closed",
            parentheses[parentheses.length - 1]
        )
    }
    if (blocks.length > 0) {
        const open = blocks[blocks.length - 1]
        return fail(
            "unbalanced-block",
            `${open.kind.toUpperCase()} block is not closed`,
            open.position
        )
    }
    if (sqliteTrigger && (!sqliteTriggerBodySeen || !sqliteTriggerClosed)) {
        return fail("unbalanced-block", "SQLite trigger body is not a balanced BEGIN/END block", firstSource ?? 0)
    }

    emit(source.length)
    return { ok: true, units }
}

export interface DatabaseSqlEditorSelection {
    from: number
    to: number
}

export type DatabaseSqlTargetRequest =
    | {
          kind: "primary"
          selection: DatabaseSqlEditorSelection
          cursor: number
      }
    | { kind: "all" }

export type DatabaseSqlTargetErrorCode =
    | DatabaseSqlParseErrorCode
    | "noExecutableStatement"
    | "noCurrentStatement"

export interface DatabaseSqlTargetError {
    code: DatabaseSqlTargetErrorCode
    position: number
    from: number
    to: number
}

export type DatabaseSqlTargetResult =
    | {
          ok: true
          units: [DatabaseSqlUnit, ...DatabaseSqlUnit[]]
          highlight: DatabaseSqlEditorSelection
      }
    | { ok: false; error: DatabaseSqlTargetError }

function targetError(
    code: DatabaseSqlTargetErrorCode,
    position: number,
    source: string
): DatabaseSqlTargetResult {
    const from = Math.max(0, Math.min(position, source.length))
    const codePoint = source.codePointAt(from)
    const to = codePoint === undefined ? from : from + (codePoint > 0xffff ? 2 : 1)
    return { ok: false, error: { code, position: from, from, to } }
}

export function resolveDatabaseSqlTarget(
    source: string,
    dialect: DatabaseSqlDialect,
    target: DatabaseSqlTargetRequest
): DatabaseSqlTargetResult {
    if (target.kind === "primary" && target.selection.from !== target.selection.to) {
        const selectionStart = Math.min(target.selection.from, target.selection.to)
        const selectionEnd = Math.max(target.selection.from, target.selection.to)
        const selected = source.slice(selectionStart, selectionEnd)
        const parsed = splitDatabaseSql(selected, dialect)
        if (!parsed.ok) return targetError(parsed.error.code, selectionStart + parsed.error.position, source)
        if (parsed.units.length === 0) {
            return targetError("noExecutableStatement", selectionStart, source)
        }
        const units = parsed.units.map((unit) => ({
            ...unit,
            start: selectionStart + unit.start,
            end: selectionStart + unit.end
        })) as [DatabaseSqlUnit, ...DatabaseSqlUnit[]]
        return {
            ok: true,
            units,
            highlight: { from: units[0].start, to: units[units.length - 1].end }
        }
    }

    if (target.kind === "primary") {
        const parsed = splitDatabaseSql(source, dialect)
        if (!parsed.ok) return targetError(parsed.error.code, parsed.error.position, source)
        if (parsed.units.length === 0) return targetError("noExecutableStatement", 0, source)
        const cursor = Math.max(0, Math.min(target.cursor, source.length))
        const containing = parsed.units.find((candidate) =>
            candidate.start <= cursor && cursor < candidate.end
        )
        let endingAtCursor: DatabaseSqlUnit | undefined
        for (let index = parsed.units.length - 1; index >= 0; index -= 1) {
            if (parsed.units[index].end === cursor) {
                endingAtCursor = parsed.units[index]
                break
            }
        }
        const unit = containing ?? endingAtCursor
        if (!unit) return targetError("noCurrentStatement", cursor, source)
        return {
            ok: true,
            units: [unit],
            highlight: { from: unit.start, to: unit.end }
        }
    }

    const parsed = splitDatabaseSql(source, dialect)
    if (!parsed.ok) return targetError(parsed.error.code, parsed.error.position, source)
    if (parsed.units.length === 0) return targetError("noExecutableStatement", 0, source)
    const units = parsed.units as [DatabaseSqlUnit, ...DatabaseSqlUnit[]]
    return {
        ok: true,
        units,
        highlight: { from: units[0].start, to: units[units.length - 1].end }
    }
}

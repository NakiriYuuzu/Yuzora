import type { DbColumn, DbKind, DbTable, DbValue } from "./types"

export function quoteDbIdentifier(kind: DbKind, value: string): string {
    if (!value.trim() || value.includes("\0")) throw new Error("invalidIdentifier")
    return kind === "mssql" ? `[${value.replace(/]/g, "]]")}]` : `"${value.replace(/"/g, '""')}"`
}

export function qualifiedDbTable(kind: DbKind, table: DbTable): string {
    const parts = kind === "mssql" ? [table.catalog, table.schema, table.name] : [table.schema, table.name]
    return parts.map(part => quoteDbIdentifier(kind, part)).join(".")
}

function textLiteral(kind: DbKind, value: string): string {
    if (value.includes("\0")) throw new Error("invalidValue")
    if (kind === "postgres") return `E'${value.replace(/\\/g, "\\\\").replace(/'/g, "''")}'`
    return `${kind === "mssql" ? "N" : ""}'${value.replace(/'/g, "''")}'`
}

export function dbValueLiteral(kind: DbKind, value: DbValue): string {
    switch (value.kind) {
        case "null": return "NULL"
        case "boolean": return kind === "postgres" ? (value.value ? "TRUE" : "FALSE") : (value.value ? "1" : "0")
        case "integer":
            if (!/^[+-]?\d+$/.test(value.value)) throw new Error("invalidValue")
            return value.value
        case "decimal":
            if (!/^[+-]?(?:\d+(?:\.\d*)?|\.\d+)(?:e[+-]?\d+)?$/i.test(value.value)) throw new Error("invalidValue")
            return value.value
        case "binary": throw new Error("readOnlyCell")
        default: return textLiteral(kind, value.value)
    }
}

export function parseEditedDbValue(original: DbValue, text: string, isNull: boolean, column: DbColumn): DbValue {
    if (isNull) {
        if (column.notnull || column.pk) throw new Error("notNullable")
        return { kind: "null" }
    }
    let kind = original.kind
    if (kind === "null") {
        const type = column.type.toLowerCase()
        kind = /bool|^bit$/.test(type) ? "boolean" : /\b(?:tiny|small|medium|big)?int(?:eger|[248])?\b/.test(type) ? "integer" : /decimal|numeric|real|float|double|money/.test(type) ? "decimal" : /json/.test(type) ? "json" : "text"
    }
    if (kind === "binary") throw new Error("readOnlyCell")
    if (kind === "boolean") {
        if (!/^(true|false|0|1)$/i.test(text.trim())) throw new Error("invalidValue")
        return { kind, value: /^(true|1)$/i.test(text.trim()) }
    }
    if (kind === "json") {
        try { JSON.parse(text) } catch { throw new Error("invalidValue") }
    }
    const value = { kind, value: kind === "integer" || kind === "decimal" ? text.trim() : text } as DbValue
    dbValueLiteral("sqlite", value)
    return value
}

/** Binary columns have no editable literal, so they stay read-only even while a
 * cell is NULL; an edit would otherwise write text into the binary column. */
export function isBinaryDbColumn(column: DbColumn): boolean {
    return /blob|bytea|binary|image/i.test(column.type)
}

// PostgreSQL resolves `real = numeric` as float8 equality, so a real cell would never
// match its own displayed value; compare in the column's own precision instead.
function predicateLiteral(kind: DbKind, column: DbColumn, value: DbValue): string {
    const literal = dbValueLiteral(kind, value)
    return kind === "postgres" && value.kind === "decimal" && /^(real|float4)$/i.test(column.type.trim()) ? `CAST(${literal} AS real)` : literal
}

// Case-insensitive or pad-space collations (common on MSSQL, opt-in on SQLite and
// PostgreSQL) would let a concurrent change such as `Alice` -> `alice` still match
// the loaded value, so the optimistic guard compares character data exactly.
function originalValuePredicate(kind: DbKind, column: DbColumn, identifier: string, original: DbValue): string {
    const literal = predicateLiteral(kind, column, original)
    if (original.kind !== "text") return `${identifier} = ${literal}`
    const type = column.type.trim().toLowerCase()
    if (kind === "sqlite") return `${identifier} = ${literal} COLLATE BINARY`
    if (kind === "postgres" && /^(?:text|character(?: varying)?)$/.test(type)) return `${identifier} = ${literal} COLLATE "C"`
    // MSSQL `=` ignores trailing spaces even under binary collations, so compare UTF-16 bytes.
    if (kind === "mssql" && /^n?(?:var)?char$|^n?text$/.test(type)) return `CAST(CAST(${identifier} AS nvarchar(max)) AS varbinary(max)) = CAST(${literal} AS varbinary(max))`
    return `${identifier} = ${literal}`
}

/** Only table previews with a complete, non-null primary key can be edited.
 * Include the original cell as an optimistic concurrency check. */
export function buildCellUpdate(kind: DbKind, table: DbTable, metadata: DbColumn[], columns: string[], row: DbValue[], columnName: string, value: DbValue): string {
    const column = metadata.find(item => item.name === columnName)
    const keys = metadata.filter(item => item.pk)
    const index = columns.indexOf(columnName)
    if (table.kind !== "table" || !column || isBinaryDbColumn(column) || keys.length === 0 || index < 0 || columns.length !== new Set(columns).size) throw new Error("readOnlyCell")
    if (value.kind === "null" && (column.notnull || column.pk)) throw new Error("notNullable")
    const predicates = keys.map(key => {
        const original = row[columns.indexOf(key.name)]
        if (!original || original.kind === "null") throw new Error("readOnlyCell")
        return `${quoteDbIdentifier(kind, key.name)} = ${predicateLiteral(kind, key, original)}`
    })
    const original = row[index]
    if (!original || original.kind === "binary" || original.kind === "json") throw new Error("readOnlyCell")
    const identifier = quoteDbIdentifier(kind, columnName)
    predicates.push(original.kind === "null" ? `${identifier} IS NULL` : originalValuePredicate(kind, column, identifier, original))
    return `UPDATE ${qualifiedDbTable(kind, table)} SET ${identifier} = ${dbValueLiteral(kind, value)} WHERE ${predicates.join(" AND ")}`
}

export type TableEdit =
    | { kind: "renameTable"; name: string }
    | { kind: "renameColumn"; column: string; name: string }
    | { kind: "addColumn"; name: string; type: string }

export const NEW_COLUMN_TYPES: Record<DbKind, string[]> = {
    sqlite: ["TEXT", "INTEGER", "REAL", "BLOB", "NUMERIC"],
    postgres: ["text", "integer", "bigint", "numeric", "boolean", "date", "timestamp", "uuid", "jsonb"],
    mssql: ["nvarchar(255)", "nvarchar(max)", "int", "bigint", "decimal(18,2)", "bit", "date", "datetime2", "uniqueidentifier"],
}

export function buildTableEdit(kind: DbKind, table: DbTable, edit: TableEdit): string {
    if (table.kind !== "table") throw new Error("readOnlyCell")
    const qualified = qualifiedDbTable(kind, table)
    const name = quoteDbIdentifier(kind, edit.name)
    if (edit.kind === "addColumn") {
        if (!NEW_COLUMN_TYPES[kind].includes(edit.type)) throw new Error("invalidValue")
        return `ALTER TABLE ${qualified} ADD ${name} ${edit.type} NULL`
    }
    if (kind === "mssql") {
        const object = [table.schema, table.name, ...(edit.kind === "renameColumn" ? [edit.column] : [])].map(part => quoteDbIdentifier(kind, part)).join(".")
        return `EXEC ${quoteDbIdentifier(kind, table.catalog)}.sys.sp_rename ${textLiteral(kind, object)}, ${textLiteral(kind, edit.name)}${edit.kind === "renameColumn" ? ", N'COLUMN'" : ""}`
    }
    return edit.kind === "renameTable"
        ? `ALTER TABLE ${qualified} RENAME TO ${name}`
        : `ALTER TABLE ${qualified} RENAME COLUMN ${quoteDbIdentifier(kind, edit.column)} TO ${name}`
}

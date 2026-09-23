import { describe, expect, it } from "vitest"
import { buildCellUpdate, buildTableEdit, dbValueLiteral, parseEditedDbValue } from "./databaseEditing"
import type { DbColumn, DbTable, DbValue } from "./types"

const table: DbTable = { catalog: "db", schema: "main", name: 'odd"table', kind: "table" }
const metadata: DbColumn[] = [{ name: "id", type: "INTEGER", pk: true, notnull: true }, { name: "name", type: "TEXT", pk: false, notnull: false }]
const row: DbValue[] = [{ kind: "integer", value: "9223372036854775807" }, { kind: "text", value: "O'Brien" }]
describe("database editing", () => {
    it("quotes identifiers and values, preserves large keys and guards the original cell", () => {
        expect(buildCellUpdate("sqlite", table, metadata, ["id", "name"], row, "name", { kind: "text", value: "'; DROP TABLE users; --" })).toBe(`UPDATE "main"."odd""table" SET "name" = '''; DROP TABLE users; --' WHERE "id" = 9223372036854775807 AND "name" = 'O''Brien'`)
    })
    it("compares PostgreSQL real cells in real precision so their displayed value matches", () => {
        const realColumns: DbColumn[] = [metadata[0], { name: "ratio", type: "real", pk: false, notnull: false }]
        expect(buildCellUpdate("postgres", table, realColumns, ["id", "ratio"], [row[0], { kind: "decimal", value: "0.1" }], "ratio", { kind: "decimal", value: "0.2" }))
            .toContain(`"ratio" = 0.2 WHERE "id" = 9223372036854775807 AND "ratio" = CAST(0.1 AS real)`)
        expect(buildCellUpdate("mssql", table, realColumns, ["id", "ratio"], [row[0], { kind: "decimal", value: "0.1" }], "ratio", { kind: "decimal", value: "0.2" }))
            .toContain(`AND [ratio] = 0.1`)
    })
    it("refuses rows without complete unique keys and ambiguous query columns", () => {
        expect(() => buildCellUpdate("sqlite", table, [], ["id", "name"], row, "name", row[1])).toThrow("readOnlyCell")
        expect(() => buildCellUpdate("sqlite", table, metadata, ["name"], [row[1]], "name", row[1])).toThrow("readOnlyCell")
        expect(() => buildCellUpdate("sqlite", { ...table, kind: "view" }, metadata, ["id", "name"], row, "name", row[1])).toThrow("readOnlyCell")
    })
    it("uses IS NULL for an original null and keeps empty strings distinct", () => {
        expect(buildCellUpdate("postgres", table, metadata, ["id", "name"], [row[0], { kind: "null" }], "name", { kind: "text", value: "" })).toContain(`"name" = E'' WHERE "id" = 9223372036854775807 AND "name" IS NULL`)
        expect(dbValueLiteral("postgres", { kind: "text", value: "\\'" })).toBe("E'\\\\'''" )
        expect(dbValueLiteral("mssql", { kind: "text", value: "中文" })).toBe("N'中文'")
    })
    it("keeps NULL cells in binary columns read-only instead of writing a text literal", () => {
        for (const [kind, type] of [["sqlite", "BLOB"], ["postgres", "bytea"], ["mssql", "varbinary"], ["mssql", "image"]] as const) {
            const binary: DbColumn[] = [metadata[0], { name: "payload", type, pk: false, notnull: false }]
            expect(() => buildCellUpdate(kind, table, binary, ["id", "payload"], [row[0], { kind: "null" }], "payload", { kind: "text", value: "abc" })).toThrow("readOnlyCell")
        }
    })
    it("validates numeric, boolean and nullable values without JS numeric conversion", () => {
        expect(parseEditedDbValue(row[0], "9223372036854775806", false, metadata[0])).toEqual({ kind: "integer", value: "9223372036854775806" })
        expect(() => parseEditedDbValue(row[0], "1; DELETE", false, metadata[0])).toThrow("invalidValue")
        expect(() => parseEditedDbValue(row[0], "", true, metadata[0])).toThrow("notNullable")
        expect(() => parseEditedDbValue({ kind: "boolean", value: true }, "yes", false, metadata[0])).toThrow("invalidValue")
    })
    it("infers a null cell's value kind from integer type names rather than substrings", () => {
        const nullable = (type: string): DbColumn => ({ name: "value", type, pk: false, notnull: false })
        expect(parseEditedDbValue({ kind: "null" }, "1 day", false, nullable("interval"))).toEqual({ kind: "text", value: "1 day" })
        expect(parseEditedDbValue({ kind: "null" }, "(1,2)", false, nullable("point"))).toEqual({ kind: "text", value: "(1,2)" })
        expect(parseEditedDbValue({ kind: "null" }, "[1,5)", false, nullable("int4range"))).toEqual({ kind: "text", value: "[1,5)" })
        for (const type of ["int", "INTEGER", "int4", "bigint", "SMALLINT", "unsigned big int"]) {
            expect(parseEditedDbValue({ kind: "null" }, " 42 ", false, nullable(type))).toEqual({ kind: "integer", value: "42" })
        }
        expect(() => parseEditedDbValue({ kind: "null" }, "1 day", false, nullable("bigint"))).toThrow("invalidValue")
    })
    it("builds dialect-specific schema operations and restricts new-column types", () => {
        expect(buildTableEdit("mssql", table, { kind: "renameColumn", column: "old]name", name: "new'column" })).toBe(`EXEC [db].sys.sp_rename N'[main].[odd"table].[old]]name]', N'new''column', N'COLUMN'`)
        expect(buildTableEdit("sqlite", table, { kind: "addColumn", name: "description", type: "TEXT" })).toContain('ADD "description" TEXT NULL')
        expect(() => buildTableEdit("sqlite", table, { kind: "addColumn", name: "x", type: "TEXT; DROP TABLE x" })).toThrow("invalidValue")
    })
})

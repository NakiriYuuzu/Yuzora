import { describe, expect, it } from "vitest"
import { buildCellUpdate, buildTableEdit, dbValueLiteral, parseEditedDbValue } from "./databaseEditing"
import type { DbColumn, DbTable, DbValue } from "./types"

const table: DbTable = { catalog: "db", schema: "main", name: 'odd"table', kind: "table" }
const metadata: DbColumn[] = [{ name: "id", type: "INTEGER", pk: true, notnull: true }, { name: "name", type: "TEXT", pk: false, notnull: false }]
const row: DbValue[] = [{ kind: "integer", value: "9223372036854775807" }, { kind: "text", value: "O'Brien" }]
describe("database editing", () => {
    it("quotes identifiers and values, preserves large keys and guards the original cell", () => {
        expect(buildCellUpdate("sqlite", table, metadata, ["id", "name"], row, "name", { kind: "text", value: "'; DROP TABLE users; --" })).toBe(`UPDATE "main"."odd""table" SET "name" = '''; DROP TABLE users; --' WHERE "id" = 9223372036854775807 AND "name" = 'O''Brien' COLLATE BINARY`)
    })
    it("guards original character data exactly so collation-equal concurrent changes conflict", () => {
        const update = (kind: "postgres" | "mssql", type: string, original: DbValue) =>
            buildCellUpdate(kind, table, [metadata[0], { name: "name", type, pk: false, notnull: false }], ["id", "name"], [row[0], original], "name", { kind: "text", value: "x" })
        expect(update("mssql", "nvarchar", row[1])).toContain(`AND CAST(CAST([name] AS nvarchar(max)) AS varbinary(max)) = CAST(N'O''Brien' AS varbinary(max))`)
        expect(update("mssql", "ntext", row[1])).toContain(`AND CAST(CAST([name] AS nvarchar(max)) AS varbinary(max)) = CAST(N'O''Brien' AS varbinary(max))`)
        expect(update("postgres", "character varying", row[1])).toContain(`AND "name" = E'O''Brien' COLLATE "C"`)
        // Non-character text values keep typed equality; collations do not apply to them.
        const guid = { kind: "text", value: "0f8fad5b-d9cb-469f-a165-70867728950e" } as const
        expect(update("mssql", "uniqueidentifier", guid)).toMatch(/AND \[name\] = N'0f8fad5b-d9cb-469f-a165-70867728950e'$/)
        expect(update("postgres", "uuid", guid)).toMatch(/AND "name" = E'0f8fad5b-d9cb-469f-a165-70867728950e'$/)
    })
    it("guards textual primary keys exactly so a collation-equal key change conflicts", () => {
        const keyed: DbColumn[] = [{ name: "code", type: "nvarchar", pk: true, notnull: true }, { name: "note", type: "nvarchar", pk: false, notnull: false }]
        const values: DbValue[] = [{ kind: "text", value: "Alice" }, { kind: "text", value: "n" }]
        expect(buildCellUpdate("mssql", table, keyed, ["code", "note"], values, "note", { kind: "text", value: "x" }))
            .toContain(`WHERE CAST(CAST([code] AS nvarchar(max)) AS varbinary(max)) = CAST(N'Alice' AS varbinary(max)) AND`)
        const sqliteKeyed: DbColumn[] = [{ name: "code", type: "TEXT", pk: true, notnull: true }, { name: "note", type: "TEXT", pk: false, notnull: false }]
        expect(buildCellUpdate("sqlite", table, sqliteKeyed, ["code", "note"], values, "note", { kind: "text", value: "x" }))
            .toContain(`WHERE "code" = 'Alice' COLLATE BINARY AND`)
    })
    it("compares PostgreSQL user-defined text types such as citext exactly", () => {
        // information_schema reports citext, domains and enums as USER-DEFINED.
        const keyed: DbColumn[] = [{ name: "email", type: "USER-DEFINED", pk: true, notnull: true }, { name: "note", type: "USER-DEFINED", pk: false, notnull: false }]
        const values: DbValue[] = [{ kind: "text", value: "Alice@x" }, { kind: "text", value: "n" }]
        const sql = buildCellUpdate("postgres", table, keyed, ["email", "note"], values, "note", { kind: "text", value: "x" })
        expect(sql).toContain(`WHERE CAST("email" AS text) COLLATE "C" = E'Alice@x' AND CAST("note" AS text) COLLATE "C" = E'n'`)
    })
    it("keeps PostgreSQL special numeric values editable as typed literals", () => {
        const keyed: DbColumn[] = [{ name: "id", type: "numeric", pk: true, notnull: true }, { name: "v", type: "numeric", pk: false, notnull: false }]
        const values: DbValue[] = [{ kind: "decimal", value: "NaN" }, { kind: "decimal", value: "-Infinity" }]
        expect(buildCellUpdate("postgres", table, keyed, ["id", "v"], values, "v", { kind: "decimal", value: "1.5" }))
            .toBe(`UPDATE "main"."odd""table" SET "v" = 1.5 WHERE "id" = 'NaN'::numeric AND "v" = '-Infinity'::numeric`)
        expect(() => dbValueLiteral("mssql", { kind: "decimal", value: "NaN" })).toThrow("invalidValue")
    })
    it.each([
        ["real", "real"], ["float4", "real"],
        ["double precision", "float8"], ["float8", "float8"],
        ["numeric", "numeric"],
    ])("formats PostgreSQL %s specials in SET, original guards and primary keys", (type, sqlType) => {
        const keyed: DbColumn[] = [{ name: "id", type, pk: true, notnull: true }, { name: "v", type, pk: false, notnull: false }]
        for (const [raw, normalized] of [["inf", "Infinity"], ["-inf", "-Infinity"], ["Infinity", "Infinity"], ["-Infinity", "-Infinity"], ["+Infinity", "Infinity"], ["NaN", "NaN"]]) {
            const special: DbValue = { kind: "decimal", value: raw }
            const literal = `'${normalized}'::${sqlType}`
            expect(buildCellUpdate("postgres", table, keyed, ["id", "v"], [special, special], "v", special))
                .toBe(`UPDATE "main"."odd""table" SET "v" = ${literal} WHERE "id" = ${literal} AND "v" = ${literal}`)
            const finite = parseEditedDbValue("postgres", special, "1.5", false, keyed[1])
            expect(buildCellUpdate("postgres", table, keyed, ["id", "v"], [special, special], "v", finite))
                .toBe(`UPDATE "main"."odd""table" SET "v" = 1.5 WHERE "id" = ${literal} AND "v" = ${literal}`)
            // A special primary key must also allow editing an ordinary text column.
            expect(buildCellUpdate("postgres", table, [keyed[0], metadata[1]], ["id", "name"], [special, row[1]], "name", { kind: "text", value: "x" }))
                .toBe(`UPDATE "main"."odd""table" SET "name" = E'x' WHERE "id" = ${literal} AND "name" = E'O''Brien' COLLATE "C"`)
        }
    })
    it("uses each PostgreSQL column's own special type, including an edited primary key", () => {
        const keyed: DbColumn[] = [{ name: "id", type: "real", pk: true, notnull: true }, { name: "v", type: "double precision", pk: false, notnull: false }]
        const values: DbValue[] = [{ kind: "decimal", value: "inf" }, { kind: "decimal", value: "-inf" }]
        const value = parseEditedDbValue("postgres", values[1], "NaN", false, keyed[1])
        expect(buildCellUpdate("postgres", table, keyed, ["id", "v"], values, "v", value))
            .toBe(`UPDATE "main"."odd""table" SET "v" = 'NaN'::float8 WHERE "id" = 'Infinity'::real AND "v" = '-Infinity'::float8`)
        expect(buildCellUpdate("postgres", table, keyed, ["id", "v"], values, "id", value))
            .toBe(`UPDATE "main"."odd""table" SET "id" = 'NaN'::real WHERE "id" = 'Infinity'::real AND "id" = 'Infinity'::real`)
    })
    it.each([
        ["real", "real"], ["float4", "real"],
        ["double precision", "float8"], ["float8", "float8"],
        ["numeric", "numeric"],
    ])("parses manual PostgreSQL %s specials, including original NULL cells", (type, sqlType) => {
        const column: DbColumn = { name: "v", type, pk: false, notnull: false }
        for (const [text, normalized] of [["inf", "Infinity"], ["-inf", "-Infinity"], ["Infinity", "Infinity"], ["-Infinity", "-Infinity"], ["NaN", "NaN"], ["+inf", "Infinity"], ["nan", "NaN"], ["infinity", "Infinity"]]) {
            for (const original of [{ kind: "decimal", value: "1.5" }, { kind: "null" }] as DbValue[]) {
                const value = parseEditedDbValue("postgres", original, ` ${text} `, false, column)
                expect(value).toEqual({ kind: "decimal", value: text })
                const guard = original.kind === "null" ? '"v" IS NULL' : `"v" = ${sqlType === "real" ? "CAST(1.5 AS real)" : "1.5"}`
                expect(buildCellUpdate("postgres", table, [metadata[0], column], ["id", "v"], [row[0], original], "v", value))
                    .toBe(`UPDATE "main"."odd""table" SET "v" = '${normalized}'::${sqlType} WHERE "id" = 9223372036854775807 AND ${guard}`)
            }
        }
    })
    it("compares PostgreSQL real cells in real precision so their displayed value matches", () => {
        const realColumns: DbColumn[] = [metadata[0], { name: "ratio", type: "real", pk: false, notnull: false }]
        expect(buildCellUpdate("postgres", table, realColumns, ["id", "ratio"], [row[0], { kind: "decimal", value: "0.1" }], "ratio", { kind: "decimal", value: "0.2" }))
            .toContain(`"ratio" = 0.2 WHERE "id" = 9223372036854775807 AND "ratio" = CAST(0.1 AS real)`)
        expect(buildCellUpdate("mssql", table, realColumns, ["id", "ratio"], [row[0], { kind: "decimal", value: "0.1" }], "ratio", { kind: "decimal", value: "0.2" }))
            .toContain(`AND [ratio] = 0.1`)
    })
    it.each(["real", "float4"])("keeps finite PostgreSQL %s primary keys in their own precision", type => {
        const keyed: DbColumn[] = [{ ...metadata[0], type }, metadata[1]]
        expect(buildCellUpdate("postgres", table, keyed, ["id", "name"], [{ kind: "decimal", value: "0.1" }, row[1]], "name", { kind: "text", value: "x" }))
            .toContain('WHERE "id" = CAST(0.1 AS real) AND')
    })
    it.each(["sqlite", "mssql"] as const)("rejects special numeric inputs and original values on %s", engine => {
        for (const type of ["real", "float8", "numeric"]) {
            const columns: DbColumn[] = [{ ...metadata[0], type }, { name: "v", type, pk: false, notnull: false }]
            const finite: DbValue = { kind: "decimal", value: "1.5" }
            for (const text of ["inf", "-inf", "+inf", "Infinity", "-Infinity", "+Infinity", "NaN"]) {
                const special: DbValue = { kind: "decimal", value: text }
                expect(() => parseEditedDbValue(engine, finite, text, false, columns[1])).toThrow("invalidValue")
                expect(() => parseEditedDbValue(engine, { kind: "null" }, text, false, columns[1])).toThrow("invalidValue")
                expect(() => buildCellUpdate(engine, table, columns, ["id", "v"], [finite, finite], "v", special)).toThrow("invalidValue")
                expect(() => buildCellUpdate(engine, table, columns, ["id", "v"], [finite, special], "v", finite)).toThrow("invalidValue")
                expect(() => buildCellUpdate(engine, table, columns, ["id", "v"], [special, finite], "v", finite)).toThrow("invalidValue")
            }
        }
    })
    it.each(["postgres", "sqlite", "mssql"] as const)("rejects malformed numeric literals on %s without changing exact finite values", engine => {
        const column: DbColumn = { name: "v", type: "numeric", pk: false, notnull: false }
        const original: DbValue = { kind: "decimal", value: "0.1" }
        for (const text of ["inf; DROP TABLE x", "Infinity'::real; --", "-NaN", "NaN()", "1 OR 1=1", "1.2.3", "1e", "Infinityx", ""]) {
            expect(() => parseEditedDbValue(engine, original, text, false, column)).toThrow("invalidValue")
            expect(() => buildCellUpdate(engine, table, [metadata[0], column], ["id", "v"], [row[0], original], "v", { kind: "decimal", value: text })).toThrow("invalidValue")
        }
        for (const text of ["9223372036854775807.1234567890123456789", "-1.234567890123456789e+100", "+.5", "1."]) {
            const parsed = parseEditedDbValue(engine, original, ` ${text} `, false, column)
            expect(parsed).toEqual({ kind: "decimal", value: text })
            expect(buildCellUpdate(engine, table, [metadata[0], column], ["id", "v"], [row[0], original], "v", parsed)).toContain(`= ${text} WHERE`)
        }
        expect(parseEditedDbValue(engine, row[0], "9223372036854775806", false, metadata[0])).toEqual({ kind: "integer", value: "9223372036854775806" })
        expect(() => parseEditedDbValue(engine, row[0], "Infinity", false, metadata[0])).toThrow("invalidValue")
        expect(() => parseEditedDbValue(engine, original, "", true, { ...column, pk: true })).toThrow("notNullable")
        expect(() => parseEditedDbValue(engine, original, "", true, { ...column, notnull: true })).toThrow("notNullable")
        expect(parseEditedDbValue(engine, original, "", true, column)).toEqual({ kind: "null" })
        expect(() => parseEditedDbValue(engine, { kind: "binary", hex: "00" }, "Infinity", false, column)).toThrow("readOnlyCell")
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
        expect(parseEditedDbValue("sqlite", row[0], "9223372036854775806", false, metadata[0])).toEqual({ kind: "integer", value: "9223372036854775806" })
        expect(() => parseEditedDbValue("sqlite", row[0], "1; DELETE", false, metadata[0])).toThrow("invalidValue")
        expect(() => parseEditedDbValue("sqlite", row[0], "", true, metadata[0])).toThrow("notNullable")
        expect(() => parseEditedDbValue("sqlite", { kind: "boolean", value: true }, "yes", false, metadata[0])).toThrow("invalidValue")
    })
    it("infers a null cell's value kind from integer type names rather than substrings", () => {
        const nullable = (type: string): DbColumn => ({ name: "value", type, pk: false, notnull: false })
        expect(parseEditedDbValue("sqlite", { kind: "null" }, "1 day", false, nullable("interval"))).toEqual({ kind: "text", value: "1 day" })
        expect(parseEditedDbValue("sqlite", { kind: "null" }, "(1,2)", false, nullable("point"))).toEqual({ kind: "text", value: "(1,2)" })
        expect(parseEditedDbValue("sqlite", { kind: "null" }, "[1,5)", false, nullable("int4range"))).toEqual({ kind: "text", value: "[1,5)" })
        for (const type of ["int", "INTEGER", "int4", "bigint", "SMALLINT", "unsigned big int"]) {
            expect(parseEditedDbValue("sqlite", { kind: "null" }, " 42 ", false, nullable(type))).toEqual({ kind: "integer", value: "42" })
        }
        expect(() => parseEditedDbValue("sqlite", { kind: "null" }, "1 day", false, nullable("bigint"))).toThrow("invalidValue")
    })
    it("builds dialect-specific schema operations and restricts new-column types", () => {
        expect(buildTableEdit("mssql", table, { kind: "renameColumn", column: "old]name", name: "new'column" })).toBe(`EXEC [db].sys.sp_rename N'[main].[odd"table].[old]]name]', N'new''column', N'COLUMN'`)
        expect(buildTableEdit("sqlite", table, { kind: "addColumn", name: "description", type: "TEXT" })).toContain('ADD "description" TEXT NULL')
        expect(() => buildTableEdit("sqlite", table, { kind: "addColumn", name: "x", type: "TEXT; DROP TABLE x" })).toThrow("invalidValue")
    })
})

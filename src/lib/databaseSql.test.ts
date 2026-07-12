import { describe, expect, it } from "vitest"

import {
    buildTableQuery,
    databaseErrorSelection,
    databaseErrorSelectionForEditor,
    dbObjectRefKey,
    resolveDatabaseSqlTarget,
    splitDatabaseSql
} from "./databaseSql"
import type { DatabaseSqlDialect, DatabaseSqlUnit } from "./databaseSql"
import type { DbError, DbObjectReference } from "./types"

function units(source: string, dialect: DatabaseSqlDialect): DatabaseSqlUnit[] {
    const result = splitDatabaseSql(source, dialect)
    expect(result.ok).toBe(true)
    if (!result.ok) throw new Error(result.error.message)
    for (const unit of result.units) expect(unit.sql).toBe(source.slice(unit.start, unit.end))
    return result.units
}

const object: DbObjectReference = {
    catalog: "app]catalog",
    schema: 'audit"schema',
    name: 'order]"items',
    kind: "table"
}

function positionedError(
    engine: DbError["engine"],
    position: DbError["position"]
): DbError {
    return {
        engine,
        message: "bad SQL",
        code: null,
        position,
        detail: null,
        hint: null,
        retryability: "notRetryable"
    }
}

describe("qualified database object helpers", () => {
    it("uses every identity field in one stable object key", () => {
        expect(dbObjectRefKey(object)).toBe(
            JSON.stringify(["app]catalog", 'audit"schema', "table", 'order]"items'])
        )
        expect(dbObjectRefKey({ ...object, schema: "other" })).not.toBe(dbObjectRefKey(object))
    })

    it("quotes each executable PostgreSQL/SQLite segment and keeps catalog identity-only", () => {
        expect(buildTableQuery("postgres", object)).toBe(
            'SELECT * FROM "audit""schema"."order]""items" LIMIT 100'
        )
        expect(buildTableQuery("sqlite", object)).toBe(
            'SELECT * FROM "audit""schema"."order]""items" LIMIT 100'
        )
    })

    it("quotes MSSQL catalog, schema, and name independently", () => {
        expect(buildTableQuery("mssql", object)).toBe(
            'SELECT TOP 100 * FROM [app]]catalog].[audit"schema].[order]]"items]'
        )
    })
})

describe("database engine error locations", () => {
    const sql = "雪😀x\nsecond"

    it("maps PostgreSQL 1-based Unicode characters to UTF-16", () => {
        expect(databaseErrorSelection(
            sql,
            positionedError("postgres", { offset: 2, line: null, column: null })
        )).toEqual({ from: 1, to: 3 })
    })

    it("maps SQLite 0-based UTF-8 bytes and rejects a mid-codepoint offset", () => {
        expect(databaseErrorSelection(
            sql,
            positionedError("sqlite", { offset: 3, line: null, column: null })
        )).toEqual({ from: 1, to: 3 })
        expect(databaseErrorSelection(
            sql,
            positionedError("sqlite", { offset: 4, line: null, column: null })
        )).toBeNull()
    })

    it("maps an MSSQL 1-based line to that line's UTF-16 start", () => {
        expect(databaseErrorSelection(
            sql,
            positionedError("mssql", { offset: null, line: 2, column: null })
        )).toEqual({ from: 5, to: 6 })
    })

    it("returns null for missing, unsupported, or out-of-range positions", () => {
        expect(databaseErrorSelection(sql, positionedError("yuzora", null))).toBeNull()
        expect(databaseErrorSelection(
            sql,
            positionedError("postgres", { offset: 99, line: null, column: null })
        )).toBeNull()
    })

    it("refuses to locate a stale error against edited SQL", () => {
        const error = positionedError("postgres", { offset: 2, line: null, column: null })
        expect(databaseErrorSelectionForEditor(sql, sql, error)).toEqual({ from: 1, to: 3 })
        expect(databaseErrorSelectionForEditor(`${sql} edited`, sql, error)).toBeNull()
    })

    it("keeps leading and trailing whitespace in the Unicode position basis", () => {
        const paddedSql = "  雪😀x  \n"
        const error = positionedError("postgres", { offset: 4, line: null, column: null })
        expect(databaseErrorSelectionForEditor(paddedSql, paddedSql, error)).toEqual({
            from: 3,
            to: 5
        })
    })
})

describe("splitDatabaseSql common and SQLite syntax", () => {
    it("keeps quoted semicolons, quoted comment markers, and comments inside exact units", () => {
        const source = [
            "  -- leading ; comment",
            "SELECT '-- ; it''s text' AS value, \"semi;--identifier\" FROM [table;name];",
            "/* between ; -- */ SELECT `odd;``name` FROM data;  "
        ].join("\n")

        const result = units(source, "sqlite")
        expect(result.map((unit) => unit.sql)).toEqual([
            "-- leading ; comment\nSELECT '-- ; it''s text' AS value, \"semi;--identifier\" FROM [table;name];",
            "/* between ; -- */ SELECT `odd;``name` FROM data;"
        ])
        expect(result[0].start).toBe(2)
        expect(result[1].end).toBe(source.trimEnd().length)
    })

    it("ignores empty and comment-only fragments", () => {
        const source = "; -- no statement\n; SELECT 1; /* trailing only */"
        expect(units(source, "sqlite").map((unit) => unit.sql)).toEqual(["SELECT 1;"])
    })

    it("keeps a SQLite trigger body as one execution unit", () => {
        const source = [
            "CREATE TRIGGER audit_update AFTER UPDATE ON items",
            "BEGIN",
            "  INSERT INTO audit(value) VALUES (CASE WHEN new.value = ';' THEN 1 ELSE 0 END);",
            "  UPDATE counters SET value = value + 1;",
            "END;",
            "SELECT 1;"
        ].join("\n")

        const result = units(source, "sqlite")
        expect(result).toHaveLength(2)
        expect(result[0].sql).toContain("INSERT INTO audit")
        expect(result[0].sql).toContain("UPDATE counters")
        expect(result[0].sql.endsWith("END;")).toBe(true)
        expect(result[1].sql).toBe("SELECT 1;")
    })
})

describe("splitDatabaseSql PostgreSQL syntax", () => {
    it("keeps tagged dollar-quoted function bodies and untagged dollar strings intact", () => {
        const source = [
            "CREATE FUNCTION public.bump(value integer)",
            "RETURNS integer",
            "LANGUAGE plpgsql",
            "AS $body$",
            "BEGIN",
            "  -- semicolon in the function body must not split",
            "  RETURN value + 1;",
            "END;",
            "$body$;",
            "SELECT $$literal ; -- still literal$$ AS body;"
        ].join("\n")

        const result = units(source, "postgres")
        expect(result).toHaveLength(2)
        expect(result[0].sql).toContain("RETURN value + 1;\nEND;")
        expect(result[0].sql.endsWith("$body$;")).toBe(true)
        expect(result[1].sql).toBe("SELECT $$literal ; -- still literal$$ AS body;")
    })

    it("supports nested PostgreSQL block comments and E-prefixed strings", () => {
        const source = "/* outer ; /* inner ; */ done */ SELECT E'one\\\\two;three'; SELECT 2;"
        expect(units(source, "postgres").map((unit) => unit.sql)).toEqual([
            "/* outer ; /* inner ; */ done */ SELECT E'one\\\\two;three';",
            "SELECT 2;"
        ])
    })

    it("keeps P6-owned row-result/DML and explicit transaction sequences ordered", () => {
        const script = [
            "BEGIN;",
            "SELECT '-- quoted marker; still data' AS value;",
            "UPDATE side_effect_counter SET value = value + 1;",
            "COMMIT;"
        ].join("\n")
        expect(units(script, "postgres").map((unit) => unit.sql)).toEqual([
            "BEGIN;",
            "SELECT '-- quoted marker; still data' AS value;",
            "UPDATE side_effect_counter SET value = value + 1;",
            "COMMIT;"
        ])
    })
})

describe("splitDatabaseSql MSSQL syntax", () => {
    it("uses standalone GO lines as separators without returning GO as SQL", () => {
        const source = [
            "SELECT 'GO; still text' AS value;",
            "GO",
            "-- GO",
            "SELECT [semi;]]identifier] FROM data;",
            "go -- batch boundary"
        ].join("\n")

        const result = units(source, "mssql")
        expect(result.map((unit) => unit.sql)).toEqual([
            "SELECT 'GO; still text' AS value;",
            "-- GO\nSELECT [semi;]]identifier] FROM data;"
        ])
        expect(result.every((unit) => !/^\s*GO\s*$/im.test(unit.sql))).toBe(true)
    })

    it("does not split semicolons inside nested BEGIN/END or CASE/END blocks", () => {
        const source = [
            "BEGIN",
            "  SELECT CASE WHEN 1 = 1 THEN 'yes;still' ELSE 'no' END;",
            "  BEGIN",
            "    SELECT 2;",
            "  END;",
            "END;",
            "SELECT 3;"
        ].join("\n")

        const result = units(source, "mssql")
        expect(result).toHaveLength(2)
        expect(result[0].sql.startsWith("BEGIN\n")).toBe(true)
        expect(result[0].sql.endsWith("END;")).toBe(true)
        expect(result[1].sql).toBe("SELECT 3;")
    })

    it("keeps a procedure definition together until GO", () => {
        const source = [
            "CREATE OR ALTER PROCEDURE dbo.refresh_items",
            "AS",
            "BEGIN",
            "  UPDATE items SET refreshed = 1;",
            "  SELECT * FROM items;",
            "END;",
            "GO",
            "SELECT 4;"
        ].join("\n")

        const result = units(source, "mssql")
        expect(result).toHaveLength(2)
        expect(result[0].sql).toContain("UPDATE items SET refreshed = 1;")
        expect(result[0].sql.endsWith("END;")).toBe(true)
        expect(result[1].sql).toBe("SELECT 4;")
    })

    it("does not confuse BEGIN TRANSACTION with a BEGIN/END block", () => {
        const source = "BEGIN TRANSACTION;\nUPDATE items SET value = 1;\nCOMMIT;"
        expect(units(source, "mssql").map((unit) => unit.sql)).toEqual([
            "BEGIN TRANSACTION;",
            "UPDATE items SET value = 1;",
            "COMMIT;"
        ])
    })
})

describe("splitDatabaseSql fail-closed errors", () => {
    it.each([
        ["sqlite", "SELECT 'unterminated", "unterminated-string"],
        ["sqlite", "SELECT \"unterminated", "unterminated-identifier"],
        ["sqlite", "SELECT [unterminated", "unterminated-identifier"],
        ["sqlite", "SELECT 1 /* unterminated", "unterminated-comment"],
        ["sqlite", "SELECT (1;", "unbalanced-parenthesis"],
        ["postgres", "SELECT 1);", "unexpected-closing-parenthesis"],
        ["postgres", "SELECT $tag$unterminated;", "unterminated-dollar-quote"],
        ["mssql", "BEGIN\nSELECT 1;", "unbalanced-block"]
    ] as const)("rejects %s input with %s", (dialect, source, code) => {
        const result = splitDatabaseSql(source, dialect)
        expect(result.ok).toBe(false)
        if (result.ok) throw new Error("expected a parse error")
        expect(result.error.code).toBe(code)
        expect(result.error.position).toBeGreaterThanOrEqual(0)
        expect("units" in result).toBe(false)
    })

    it("returns no partial units when a later construct is unbalanced", () => {
        const result = splitDatabaseSql("UPDATE accounts SET active = 1; SELECT 'open", "sqlite")
        expect(result.ok).toBe(false)
        if (result.ok) throw new Error("expected a parse error")
        expect(result.error.code).toBe("unterminated-string")
        expect("units" in result).toBe(false)
    })

    it("rejects ambiguous MSSQL GO variants before exposing earlier units", () => {
        for (const separator of ["GO 2", "GO;"]) {
            const result = splitDatabaseSql(`UPDATE accounts SET active = 1;\n${separator}\nSELECT 2;`, "mssql")
            expect(result.ok).toBe(false)
            if (result.ok) throw new Error("expected a parse error")
            expect(result.error.code).toBe("ambiguous-batch-separator")
            expect("units" in result).toBe(false)
        }
    })

    it("rejects PostgreSQL plain-string backslashes whose quote semantics are server-dependent", () => {
        const result = splitDatabaseSql("SELECT 'C:\\temp;value'; SELECT 2;", "postgres")
        expect(result.ok).toBe(false)
        if (result.ok) throw new Error("expected a parse error")
        expect(result.error.code).toBe("ambiguous-string-escape")
    })

    it("rejects a GO boundary that bisects a T-SQL block", () => {
        const result = splitDatabaseSql("BEGIN\nSELECT 1;\nGO\nEND;", "mssql")
        expect(result.ok).toBe(false)
        if (result.ok) throw new Error("expected a parse error")
        expect(result.error.code).toBe("unbalanced-block")
    })
})

describe("resolveDatabaseSqlTarget", () => {
    it("parses an exact selection and rebases its units to editor offsets", () => {
        const source = "SELECT 0;\n  SELECT 'one;two'; SELECT 2;"
        const from = source.indexOf("SELECT 'one;two'")
        const to = source.indexOf(" SELECT 2")

        const result = resolveDatabaseSqlTarget(source, "sqlite", {
            kind: "primary",
            selection: { from, to },
            cursor: from
        })

        expect(result).toEqual({
            ok: true,
            units: [{
                sql: "SELECT 'one;two';",
                start: from,
                end: to,
                transactionBoundary: "none"
            }],
            highlight: { from, to }
        })
    })

    it("runs only the exact statement containing an unselected cursor", () => {
        const source = "SELECT 1;\nSELECT 2;"
        const secondStart = source.indexOf("SELECT 2")

        const result = resolveDatabaseSqlTarget(source, "sqlite", {
            kind: "primary",
            selection: { from: secondStart + 3, to: secondStart + 3 },
            cursor: secondStart + 3
        })

        expect(result).toEqual({
            ok: true,
            units: [{
                sql: "SELECT 2;",
                start: secondStart,
                end: source.length,
                transactionBoundary: "none"
            }],
            highlight: { from: secondStart, to: source.length }
        })
    })

    it("resolves Run All as an ordered script with top-level transaction metadata", () => {
        const source = "BEGIN;\nSELECT 1;\nCOMMIT;"

        const result = resolveDatabaseSqlTarget(source, "postgres", { kind: "all" })

        expect(result.ok).toBe(true)
        if (!result.ok) throw new Error(result.error.code)
        expect(result.units.map(({ sql, transactionBoundary }) => ({ sql, transactionBoundary }))).toEqual([
            { sql: "BEGIN;", transactionBoundary: "begin" },
            { sql: "SELECT 1;", transactionBoundary: "none" },
            { sql: "COMMIT;", transactionBoundary: "commit" }
        ])
        expect(result.highlight).toEqual({ from: 0, to: source.length })
    })

    it("fails closed when an unselected cursor is in the gap between statements", () => {
        const source = "SELECT 1;\n\nSELECT 2;"
        const cursor = source.indexOf("\n\n") + 1

        expect(resolveDatabaseSqlTarget(source, "sqlite", {
            kind: "primary",
            selection: { from: cursor, to: cursor },
            cursor
        })).toEqual({
            ok: false,
            error: { code: "noCurrentStatement", position: cursor, from: cursor, to: cursor + 1 }
        })
    })

    it("treats the caret immediately after a final semicolon as the final statement", () => {
        const source = "SELECT 1;"

        const result = resolveDatabaseSqlTarget(source, "sqlite", {
            kind: "primary",
            selection: { from: source.length, to: source.length },
            cursor: source.length
        })

        expect(result.ok).toBe(true)
        if (!result.ok) throw new Error(result.error.code)
        expect(result.units.map((unit) => unit.sql)).toEqual([source])
    })

    it("rebases a selected parse error and exposes no executable units", () => {
        const source = "SELECT 0;\nSELECT 'unterminated"
        const from = source.indexOf("SELECT 'unterminated")
        const quote = source.indexOf("'", from)

        expect(resolveDatabaseSqlTarget(source, "sqlite", {
            kind: "primary",
            selection: { from, to: source.length },
            cursor: source.length
        })).toEqual({
            ok: false,
            error: {
                code: "unterminated-string",
                position: quote,
                from: quote,
                to: quote + 1
            }
        })
    })

    it("does not classify a T-SQL BEGIN block as a transaction boundary", () => {
        const source = "BEGIN\nSELECT 1;\nEND;\nBEGIN TRANSACTION;\nROLLBACK;"
        const result = resolveDatabaseSqlTarget(source, "mssql", { kind: "all" })

        expect(result.ok).toBe(true)
        if (!result.ok) throw new Error(result.error.code)
        expect(result.units.map((unit) => unit.transactionBoundary)).toEqual([
            "none",
            "begin",
            "rollback"
        ])
    })
})

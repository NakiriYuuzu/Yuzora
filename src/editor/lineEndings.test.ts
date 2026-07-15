import { describe, expect, it } from "vitest"
// @ts-expect-error Vitest runs this fixture under Bun; the frontend tsconfig intentionally omits Node types.
import { mkdtemp, readFile, rm, writeFile } from "node:fs/promises"
// @ts-expect-error Vitest runs this fixture under Bun; the frontend tsconfig intentionally omits Node types.
import { tmpdir } from "node:os"
// @ts-expect-error Vitest runs this fixture under Bun; the frontend tsconfig intentionally omits Node types.
import { join } from "node:path"

import { normalizeDocumentLineEndings, serializeDocumentLineEndings } from "./lineEndings"

describe("serializeDocumentLineEndings", () => {
    it("normalizes LF, CRLF, and bare CR for editor and LSP buffers", () => {
        expect(normalizeDocumentLineEndings("one\r\ntwo\rthree\n")).toBe("one\ntwo\nthree\n")
    })

    it("normalizes LF output without adding a trailing newline", () => {
        expect(serializeDocumentLineEndings("one\r\ntwo\rthree", "lf")).toEqual({
            kind: "ready",
            content: "one\ntwo\nthree"
        })
        expect(serializeDocumentLineEndings("single line", "lf")).toEqual({
            kind: "ready",
            content: "single line"
        })
    })

    it("serializes normalized editor content to CRLF exactly once", () => {
        expect(serializeDocumentLineEndings("one\ntwo\n", "crlf")).toEqual({
            kind: "ready",
            content: "one\r\ntwo\r\n"
        })
        expect(serializeDocumentLineEndings("one\r\ntwo\n", "crlf")).toEqual({
            kind: "ready",
            content: "one\r\ntwo\r\n"
        })
    })

    it("blocks mixed content without producing writable output", () => {
        expect(serializeDocumentLineEndings("one\ntwo\n", "mixed")).toEqual({
            kind: "blocked",
            reason: "mixed"
        })
    })

    it("writes exact bytes for minimally edited LF/CRLF fixtures and keeps Mixed unchanged until selection", async () => {
        const dir = await mkdtemp(join(tmpdir(), "yuzora-eol-"))
        try {
            const crlfPath = join(dir, "crlf.txt")
            const lfPath = join(dir, "lf.txt")
            const mixedPath = join(dir, "mixed.txt")
            await writeFile(crlfPath, "one\r\ntwo\r\n", "utf8")
            await writeFile(lfPath, "one\ntwo\n", "utf8")
            await writeFile(mixedPath, "one\r\ntwo\n", "utf8")

            const crlf = serializeDocumentLineEndings("one\ntwO\n", "crlf")
            const lf = serializeDocumentLineEndings("one\ntwO\n", "lf")
            const blocked = serializeDocumentLineEndings("one\ntwO\n", "mixed")
            if (crlf.kind === "ready") await writeFile(crlfPath, crlf.content, "utf8")
            if (lf.kind === "ready") await writeFile(lfPath, lf.content, "utf8")
            if (blocked.kind === "ready") await writeFile(mixedPath, blocked.content, "utf8")

            expect(Array.from(await readFile(crlfPath))).toEqual(Array.from(new TextEncoder().encode("one\r\ntwO\r\n")))
            expect(Array.from(await readFile(lfPath))).toEqual(Array.from(new TextEncoder().encode("one\ntwO\n")))
            expect(Array.from(await readFile(mixedPath))).toEqual(Array.from(new TextEncoder().encode("one\r\ntwo\n")))

            const selected = serializeDocumentLineEndings("one\ntwO\n", "crlf")
            if (selected.kind === "ready") await writeFile(mixedPath, selected.content, "utf8")
            expect(Array.from(await readFile(mixedPath))).toEqual(Array.from(new TextEncoder().encode("one\r\ntwO\r\n")))
        } finally {
            await rm(dir, { recursive: true, force: true })
        }
    })
})

import { describe, expect, it } from "vitest"

import type { DiffContent } from "@/lib/types"
import { diffStats, langLabel, splitPath } from "./diffLoad"

describe("diffLoad helpers", () => {
    it("splitPath separates name and dir", () => {
        expect(splitPath("src/a/b.ts")).toEqual({ name: "b.ts", dir: "src/a/" })
        expect(splitPath("top.ts")).toEqual({ name: "top.ts", dir: "" })
    })

    it("langLabel maps known extensions and falls back to uppercase ext", () => {
        expect(langLabel("a.ts")).toBe("TypeScript")
        expect(langLabel("a.rs")).toBe("Rust")
        // Unknown ext → uppercased ext.
        expect(langLabel("bun.lock")).toBe("LOCK")
        // Extensionless → empty.
        expect(langLabel("Makefile")).toBe("")
    })

    it("diffStats counts added/deleted lines, null for undisplayable sides", () => {
        const c = (o: string, m: string): DiffContent => ({
            original: { kind: "full", content: o },
            modified: { kind: "full", content: m }
        })
        expect(diffStats(c("a\nb\n", "a\nb\nc\n"))).toEqual({ added: 1, deleted: 0 })
        expect(diffStats(c("a\nb\n", "a\n"))).toEqual({ added: 0, deleted: 1 })
        // binary side → no stats.
        expect(diffStats({ original: { kind: "binary" }, modified: { kind: "full", content: "x" } })).toBeNull()
    })
})

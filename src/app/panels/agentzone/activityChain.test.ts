import { describe, expect, it } from "vitest"

import { toolInvocationDetail } from "./ActivityChain"

describe("toolInvocationDetail", () => {
    it("prefers command over path and joins string arrays", () => {
        expect(toolInvocationDetail({ command: "cargo test --locked" })).toBe("cargo test --locked")
        expect(toolInvocationDetail({ command: ["git", "status"] })).toBe("git status")
        expect(toolInvocationDetail({ command: "ls", path: "/tmp" })).toBe("ls")
    })

    it("falls back through path-like and query-like keys", () => {
        expect(toolInvocationDetail({ path: "src/a.ts" })).toBe("src/a.ts")
        expect(toolInvocationDetail({ file_path: "src/b.ts" })).toBe("src/b.ts")
        expect(toolInvocationDetail({ pattern: "TODO" })).toBe("TODO")
        expect(toolInvocationDetail({ url: "https://example.com" })).toBe("https://example.com")
    })

    it("returns undefined for missing, empty, or non-string values", () => {
        expect(toolInvocationDetail(undefined)).toBeUndefined()
        expect(toolInvocationDetail({})).toBeUndefined()
        expect(toolInvocationDetail({ command: "   " })).toBeUndefined()
        expect(toolInvocationDetail({ command: 42, path: [1] })).toBeUndefined()
    })
})

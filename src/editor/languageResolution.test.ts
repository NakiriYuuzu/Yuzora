import { describe, expect, it } from "vitest"
import { EditorState } from "@codemirror/state"
import { syntaxTree } from "@codemirror/language"
import { loadLanguageExtension } from "./cmExtensions"

describe("language resolution", () => {
    it.each(["src/main.mts", "C:\\src\\main.cts", "app.mjs", "app.cjs", "module.cs", "app.kt", "build.kts", "script.ps1", "module.psm1", "manifest.psd1", "app.dart", "app.scala", ".env.local", ".zshrc", ".gitconfig", "Dockerfile.dev", "Containerfile", "types.pyi", "icon.svg", "app.csproj"])("provides a tokenizer for %s", async path => {
        const extension = await loadLanguageExtension(path)
        expect(extension).not.toBeNull()
        expect(() => EditorState.create({ doc: "hello", extensions: [extension!] })).not.toThrow()
    })
    it("does not mistake a dotted folder or unsupported file for a language", async () => {
        for (const path of ["folder.ts/README", "README", "app.ex", "main.fs", "main.zig"]) expect(await loadLanguageExtension(path)).toBeNull()
    })
    it("parses TypeScript module syntax rather than plain JavaScript", async () => {
        const state = EditorState.create({ doc: "const count: number = 1", extensions: [(await loadLanguageExtension("main.mts"))!] })
        expect(syntaxTree(state).toString()).toContain("TypeAnnotation")
    })
})

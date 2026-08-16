import { describe, expect, it } from "vitest"

import {
    MARKDOWN_PREVIEW_TAB_NAME,
    isFileTab,
    isMarkdownPreviewForSource,
    isMarkdownPreviewPath,
    isMarkdownPreviewTab,
    markdownPreviewPath,
    markdownPreviewSourcePath,
    markdownPreviewTabName,
    previewTabSourcePath
} from "./markdownPreviewTab"

describe("markdownPreviewPath", () => {
    it("round-trips POSIX, Windows, Unicode and reserved characters", () => {
        const sources = [
            "/w/readme.md",
            "C:\\Work\\notes.md",
            "\\\\?\\C:\\Work\\中文 workspace\\a.md",
            "/w/100% ready?.md",
            "/w/what#next.md"
        ]
        for (const source of sources) {
            const synthetic = markdownPreviewPath(source)
            expect(isMarkdownPreviewPath(synthetic)).toBe(true)
            expect(markdownPreviewSourcePath(synthetic)).toBe(source)
        }
    })

    it("returns null for malformed or non-preview paths without throwing", () => {
        expect(markdownPreviewSourcePath("/w/readme.md")).toBeNull()
        expect(markdownPreviewSourcePath("yuzora://preview")).toBeNull()
        expect(markdownPreviewSourcePath("yuzora://markdown-preview/%E0%A4%A")).toBeNull()
    })
})

describe("tab classifiers", () => {
    it("recognizes markdown preview tabs by kind or synthetic path", () => {
        const source = "/w/r.md"
        const tab = {
            path: markdownPreviewPath(source),
            kind: "markdown-preview",
            sourcePath: source
        }
        expect(isMarkdownPreviewTab(tab)).toBe(true)
        expect(isMarkdownPreviewForSource(tab, source)).toBe(true)
        expect(isMarkdownPreviewForSource(tab, "/w/other.md")).toBe(false)
        expect(previewTabSourcePath(tab)).toBe(source)
        expect(markdownPreviewTabName()).toBe(MARKDOWN_PREVIEW_TAB_NAME)
        expect(isMarkdownPreviewTab({ path: markdownPreviewPath(source) })).toBe(true)
    })

    it("treats only real files as file tabs", () => {
        expect(isFileTab({ path: "/w/a.ts" })).toBe(true)
        expect(isFileTab({ path: "/w/a.ts", kind: "file" })).toBe(true)
        expect(isFileTab({ path: "yuzora://preview", kind: "preview" })).toBe(false)
        expect(isFileTab({
            path: markdownPreviewPath("/w/r.md"),
            kind: "markdown-preview",
            sourcePath: "/w/r.md"
        })).toBe(false)
        expect(isFileTab({ path: "yuzora://herdr/live/term-1", kind: "herdr-terminal" })).toBe(false)
        expect(isFileTab({ path: "yuzora://markdown-preview/x" })).toBe(false)
    })
})

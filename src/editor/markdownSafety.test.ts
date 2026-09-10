import { describe, expect, it } from "vitest"
import { markdownRoundTripSafe, needsMarkdownSource } from "./markdownSafety"

describe("Markdown lossless boundary", () => {
    it.each(["---\ntitle: Test\n---\n", "<script>alert(1)</script>", "hello <span>raw</span>", "![image](x.png)", "[ref]: https://example.com\n", "[^1]: note", "$$x$$", "::: custom\nblock\n:::"])("keeps advanced source unchanged: %s", (source) => {
        expect(needsMarkdownSource(source)).toBe(true)
    })
    it("accepts ordinary Markdown with semantically equivalent serialization", () => {
        expect(markdownRoundTripSafe("# Hello\n\nText **bold**.\n", "# Hello\n\nText **bold**.")).toBe(true)
    })
    it("rejects a serializer dropping source content or table alignment", () => {
        expect(markdownRoundTripSafe("a\n\nb", "a")).toBe(false)
        expect(markdownRoundTripSafe("| A |\n| :--: |\n| x |", "| A |\n| --- |\n| x |")).toBe(false)
    })
})

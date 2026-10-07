import { cleanup, fireEvent, render, screen } from "@testing-library/react"
import { afterEach, expect, it, vi } from "vitest"

const mocks = vi.hoisted(() => ({ writeText: vi.fn(async () => undefined), success: vi.fn(), error: vi.fn() }))
vi.mock("@tauri-apps/plugin-clipboard-manager", () => ({ writeText: mocks.writeText }))
vi.mock("sonner", () => ({ toast: { success: mocks.success, error: mocks.error } }))

import { MarkdownHtml, renderMarkdown } from "./MarkdownPreview"

afterEach(() => { cleanup(); vi.clearAllMocks() })

it("copies a fenced block's text without its closing newline or the control itself", async () => {
    render(<MarkdownHtml html={renderMarkdown("Intro\n\n```ts\nconst a = 1\n\nreturn a\n```\n")} />)
    fireEvent.click(screen.getByRole("button", { name: "Copy code" }))
    await vi.waitFor(() => expect(mocks.writeText).toHaveBeenCalledWith("const a = 1\n\nreturn a"))
    expect(mocks.success).toHaveBeenCalled()
})

it("keeps exactly one control per block across re-renders and removes them with the content", () => {
    const html = renderMarkdown("```\none\n```\n\n    indented\n")
    const view = render(<MarkdownHtml html={html} />)
    expect(screen.getAllByRole("button", { name: "Copy code" })).toHaveLength(2)
    view.rerender(<MarkdownHtml html={html} />)
    expect(screen.getAllByRole("button", { name: "Copy code" })).toHaveLength(2)
    view.rerender(<MarkdownHtml html={renderMarkdown("```\nonly\n```")} />)
    expect(screen.getAllByRole("button", { name: "Copy code" })).toHaveLength(1)
    view.rerender(<MarkdownHtml html={renderMarkdown("```\nonly\n```")} copyCode={false} />)
    expect(screen.queryByRole("button", { name: "Copy code" })).toBeNull()
})

it("never lets document markup supply its own copy control", () => {
    render(<MarkdownHtml html={renderMarkdown('<button class="markdown-code-copy">Copy code</button>\n\nplain')} />)
    expect(screen.queryByRole("button")).toBeNull()
})

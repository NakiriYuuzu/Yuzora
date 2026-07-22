import { describe, expect, it, vi } from "vitest"
import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react"
import { writeText } from "@tauri-apps/plugin-clipboard-manager"
import { openUrl } from "@tauri-apps/plugin-opener"

import { MinimalMarkdown } from "./markdown"

vi.mock("@tauri-apps/plugin-opener", () => ({ openUrl: vi.fn(() => Promise.resolve()) }))
vi.mock("@tauri-apps/plugin-clipboard-manager", () => ({ writeText: vi.fn(() => Promise.resolve()) }))

describe("MinimalMarkdown", () => {
  it("keeps fenced code, inline code and bold rendering", () => {
    render(<MinimalMarkdown text={"Plan **done** with `src/git.rs`\n```ts\nconst ok = true\n```"} />)
    expect(screen.getByText("done").tagName).toBe("STRONG")
    expect(screen.getByText("src/git.rs").tagName).toBe("CODE")
    expect(screen.getByText("const ok = true").closest("pre")).not.toBeNull()
  })

  // soak 回饋 2026-07-22：code block 右上角複製鈕（plugin-clipboard-manager），
  // 成功後短暫顯示已複製狀態。
  it("copies fenced code via the copy button", async () => {
    render(<MinimalMarkdown text={"```ts\nconst ok = true\nconsole.log(ok)\n```"} />)
    const button = screen.getByRole("button", { name: "Copy code" })
    fireEvent.click(button)
    expect(writeText).toHaveBeenCalledExactlyOnceWith("const ok = true\nconsole.log(ok)")
    await waitFor(() => expect(screen.getByRole("button", { name: "Copy code" })).toHaveAttribute("title", "Copied"))
  })

  it("renders one copy button per fenced block", () => {
    render(<MinimalMarkdown text={"```\na\n```\ntext between\n```\nb\n```"} />)
    expect(screen.getAllByRole("button", { name: "Copy code" })).toHaveLength(2)
  })

  it("renders headings h3-h6 for # through ####", () => {
    render(<MinimalMarkdown text={"# One\n## Two\n### Three\n#### Four"} />)
    expect(screen.getByText("One").tagName).toBe("H3")
    expect(screen.getByText("Two").tagName).toBe("H4")
    expect(screen.getByText("Three").tagName).toBe("H5")
    expect(screen.getByText("Four").tagName).toBe("H6")
  })

  it("renders unordered and ordered lists with inline markdown in items", () => {
    render(<MinimalMarkdown text={"- alpha `a`\n- beta\n\n1. first\n2) second"} />)
    const uls = screen.getAllByRole("list")
    expect(uls).toHaveLength(2)
    expect(screen.getByText("beta").tagName).toBe("LI")
    expect(screen.getByText("a").tagName).toBe("CODE")
    expect(screen.getByText("second").tagName).toBe("LI")
    expect(screen.getByText("first").closest("ol")).not.toBeNull()
  })

  it("renders blockquotes", () => {
    render(<MinimalMarkdown text={"> quoted line\n> more"} />)
    const quote = screen.getByText(/quoted line/)
    expect(quote.closest("blockquote")).not.toBeNull()
  })

  it("renders http(s) links that open through the opener plugin without navigation", () => {
    render(<MinimalMarkdown text={"see [the docs](https://example.com/a) now"} />)
    const link = screen.getByRole("link", { name: "the docs" })
    expect(link.getAttribute("href")).toBe("https://example.com/a")
    expect(link.getAttribute("rel")).toContain("noopener")
    fireEvent.click(link)
    expect(openUrl).toHaveBeenCalledWith("https://example.com/a")
  })

  it("does not linkify non-http schemes and keeps literal brackets", () => {
    render(<MinimalMarkdown text={"bad [x](javascript:alert(1)) and plain [note] text"} />)
    expect(screen.queryByRole("link")).toBeNull()
    expect(screen.getByText(/plain \[note\] text/)).toBeInTheDocument()
  })

  it("keeps plain multi-line text as pre-wrap outside of blocks", () => {
    render(<MinimalMarkdown text={"line one\nline two"} />)
    expect(screen.getByText("line one line two")).toHaveClass("whitespace-pre-wrap")
  })

  it("renders GFM tables with alignment and inline markdown in cells", () => {
    render(
      <MinimalMarkdown
        text={"| Name | Count |\n| :--- | ---: |\n| `a.ts` | 12 |\n| b.ts | 3 |"}
      />
    )
    expect(screen.getByRole("table")).toBeInTheDocument()
    expect(screen.getByRole("columnheader", { name: "Name" })).toBeInTheDocument()
    expect(screen.getByRole("cell", { name: "12" })).toHaveStyle({ textAlign: "right" })
    expect(screen.getByText("a.ts").tagName).toBe("CODE")
    expect(screen.getAllByRole("row")).toHaveLength(3)
  })

  it("does not treat pipe-free or separator-free lines as tables", () => {
    render(<MinimalMarkdown text={"a | b\nplain next line"} />)
    expect(screen.queryByRole("table")).toBeNull()
  })

  it("renders nested lists from indentation, including indented top-level items", () => {
    render(<MinimalMarkdown text={"- parent\n  - child one\n  - child two\n- sibling"} />)
    expect(screen.getAllByRole("list")).toHaveLength(2)
    const nested = screen.getByText("child one").closest("ul")
    expect(nested?.closest("li")?.textContent).toContain("parent")

    cleanup()
    render(<MinimalMarkdown text={"  - indented one\n  - indented two"} />)
    expect(screen.getAllByRole("listitem")).toHaveLength(2)
  })

  it("renders task-list checkboxes", () => {
    render(<MinimalMarkdown text={"- [x] done item\n- [ ] todo item"} />)
    expect(screen.getByText(/done item/, { selector: "li" }).textContent).toContain("✓")
    expect(screen.getByText(/todo item/, { selector: "li" }).textContent).not.toContain("✓")
  })

  it("renders thematic breaks", () => {
    const { container } = render(<MinimalMarkdown text={"above\n\n---\n\nbelow"} />)
    expect(container.querySelector("hr")).not.toBeNull()
  })

  it("renders italic and strikethrough while keeping lone stars literal", () => {
    const { container } = render(
      <MinimalMarkdown text={"an *emphasis* and ~~gone~~ plus a*b test"} />
    )
    expect(screen.getByText("emphasis").tagName).toBe("EM")
    expect(screen.getByText("gone").tagName).toBe("S")
    expect(container.textContent).toContain("a*b test")
  })
})

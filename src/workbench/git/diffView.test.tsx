import { render, screen } from "@testing-library/react"
import { describe, expect, it } from "vitest"
import { DiffView } from "./DiffView"

const full = (content: string) => ({ kind: "full" as const, content })

describe("DiffView", () => {
    it("renders placeholder for binary side", () => {
        render(
            <DiffView content={{ original: { kind: "binary" }, modified: full("x") }} mode="unified" path="a.txt" />
        )
        expect(screen.getByText("Diff unavailable")).toBeInTheDocument()
    })
    it("mounts a CodeMirror editor for unified diff", () => {
        const { container } = render(
            <DiffView content={{ original: full("one\n"), modified: full("two\n") }} mode="unified" path="a.txt" />
        )
        expect(container.querySelector(".cm-editor")).not.toBeNull()
    })
    it("shows line-number gutters for unified diff", () => {
        const { container } = render(
            <DiffView content={{ original: full("one\n"), modified: full("two\n") }} mode="unified" path="a.txt" />
        )
        expect(container.querySelector(".cm-lineNumbers")).not.toBeNull()
    })
    it("mounts two editors for split mode", () => {
        const { container } = render(
            <DiffView content={{ original: full("one\n"), modified: full("two\n") }} mode="split" path="a.txt" />
        )
        expect(container.querySelectorAll(".cm-editor").length).toBe(2)
    })
    it("shows line-number gutters on both sides for split mode", () => {
        const { container } = render(
            <DiffView content={{ original: full("one\n"), modified: full("two\n") }} mode="split" path="a.txt" />
        )
        expect(container.querySelectorAll(".cm-lineNumbers").length).toBe(2)
    })
    it("syntax-highlights unified diff when the path resolves a language", () => {
        // Identical sides → no change decorations, so any span[class] proves the
        // language facet is driving syntax highlighting (not merge markup).
        const { container } = render(
            <DiffView
                content={{ original: full("const x = 1\n"), modified: full("const x = 1\n") }}
                mode="unified"
                path="a.ts"
            />
        )
        expect(container.querySelectorAll(".cm-line span[class]").length).toBeGreaterThan(0)
    })
    it("syntax-highlights split diff when the path resolves a language", () => {
        const { container } = render(
            <DiffView
                content={{ original: full("const x = 1\n"), modified: full("const x = 1\n") }}
                mode="split"
                path="a.ts"
            />
        )
        expect(container.querySelectorAll(".cm-line span[class]").length).toBeGreaterThan(0)
    })
    it("mounts without a language for unknown extensions", () => {
        const { container } = render(
            <DiffView
                content={{ original: full("const x = 1\n"), modified: full("const y = 2\n") }}
                mode="unified"
                path="a.unknown"
            />
        )
        expect(container.querySelector(".cm-editor")).not.toBeNull()
    })
})

import { render, screen } from "@testing-library/react"
import { describe, expect, it } from "vitest"
import { DiffView } from "./DiffView"

const full = (content: string) => ({ kind: "full" as const, content })

describe("DiffView", () => {
    it("renders placeholder for binary side", () => {
        render(<DiffView content={{ original: { kind: "binary" }, modified: full("x") }} mode="unified" />)
        expect(screen.getByText("無法顯示 diff")).toBeInTheDocument()
    })
    it("mounts a CodeMirror editor for unified diff", () => {
        const { container } = render(
            <DiffView content={{ original: full("one\n"), modified: full("two\n") }} mode="unified" />
        )
        expect(container.querySelector(".cm-editor")).not.toBeNull()
    })
    it("mounts two editors for split mode", () => {
        const { container } = render(
            <DiffView content={{ original: full("one\n"), modified: full("two\n") }} mode="split" />
        )
        expect(container.querySelectorAll(".cm-editor").length).toBe(2)
    })
})

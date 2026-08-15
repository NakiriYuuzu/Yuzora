import { fireEvent, render, screen, waitFor } from "@testing-library/react"
import { describe, expect, it } from "vitest"
import { uiInitialState, useUiStore } from "@/state/uiStore"
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
    it("bounds the host and split MergeView for CodeMirror-owned scrolling", () => {
        const { container } = render(
            <DiffView content={{ original: full("one\n"), modified: full("two\n") }} mode="split" path="a.txt" />
        )
        expect(container.querySelector(".diff-view")).toHaveClass("h-full", "min-h-0", "overflow-hidden")
        expect(container.querySelector(".cm-mergeView")).not.toBeNull()
        expect(container.querySelectorAll(".cm-scroller")).toHaveLength(2)
        for (const el of container.querySelectorAll(".cm-mergeViewEditors, .cm-mergeViewEditor")) {
            expect((el as HTMLElement).style.height).not.toBe("100%")
        }
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

    it("shows no-differences feedback and disables chunk navigation", () => {
        render(
            <DiffView content={{ original: full("same\n"), modified: full("same\n") }} mode="unified" path="a.txt" />
        )
        expect(screen.getByText("No differences")).toBeInTheDocument()
        expect(screen.getByRole("button", { name: "Previous change" })).toBeDisabled()
        expect(screen.getByRole("button", { name: "Next change" })).toBeDisabled()
    })

    it("navigates chunks and wraps in unified and split modes", async () => {
        const content = { original: full("a\nb\nc\nd\n"), modified: full("A\nb\nC\nd\n") }
        const { rerender } = render(<DiffView content={content} mode="unified" path="a.txt" />)
        await waitFor(() => expect(screen.getByText("1 / 2")).toBeInTheDocument())
        fireEvent.click(screen.getByRole("button", { name: "Next change" }))
        expect(screen.getByText("2 / 2")).toBeInTheDocument()
        fireEvent.click(screen.getByRole("button", { name: "Next change" }))
        expect(screen.getByText("1 / 2")).toBeInTheDocument()
        fireEvent.click(screen.getByRole("button", { name: "Previous change" }))
        expect(screen.getByText("2 / 2")).toBeInTheDocument()

        rerender(<DiffView content={content} mode="split" path="a.txt" />)
        await waitFor(() => expect(screen.getByText("1 / 2")).toBeInTheDocument())
        fireEvent.keyDown(screen.getByTestId("diff-view-root"), { key: "F7" })
        await waitFor(() => expect(screen.getByText("2 / 2")).toBeInTheDocument())
    })

    it("keeps a session-only shared split ratio with keyboard clamps and reset", () => {
        useUiStore.setState(uiInitialState)
        render(
            <DiffView content={{ original: full("one\n"), modified: full("two\n") }} mode="split" path="a.txt" />
        )
        const sep = screen.getByRole("separator")
        expect(sep).toHaveAttribute("aria-orientation", "vertical")
        expect(sep).toHaveAttribute("aria-valuenow", "50")
        fireEvent.keyDown(sep, { key: "ArrowRight" })
        expect(useUiStore.getState().gitDiffSplitRatio).toBeCloseTo(0.52)
        fireEvent.keyDown(sep, { key: "Home" })
        expect(useUiStore.getState().gitDiffSplitRatio).toBe(0.25)
        fireEvent.keyDown(sep, { key: "End" })
        expect(useUiStore.getState().gitDiffSplitRatio).toBe(0.75)
        fireEvent.keyDown(sep, { key: "Enter" })
        expect(useUiStore.getState().gitDiffSplitRatio).toBe(0.5)
        fireEvent.doubleClick(sep)
        expect(useUiStore.getState().gitDiffSplitRatio).toBe(0.5)
    })

    it("commits the split ratio on pointer release without requiring a React move", () => {
        useUiStore.setState(uiInitialState)
        const { container } = render(
            <DiffView content={{ original: full("one\n"), modified: full("two\n") }} mode="split" path="a.txt" />
        )
        const host = container.querySelector(".diff-view") as HTMLElement
        host.getBoundingClientRect = () => ({
            x: 0, y: 0, top: 0, left: 0, right: 400, bottom: 200, width: 400, height: 200, toJSON() { return {} }
        })
        const sep = screen.getByTestId("diff-split-separator")
        fireEvent.pointerDown(sep, { clientX: 200, pointerId: 1 })
        fireEvent.pointerMove(sep, { clientX: 100, pointerId: 1 })
        expect(host.style.getPropertyValue("--yz-diff-split-a")).toBe("0.25")
        fireEvent.pointerUp(sep, { pointerId: 1 })
        expect(useUiStore.getState().gitDiffSplitRatio).toBe(0.25)
    })
})

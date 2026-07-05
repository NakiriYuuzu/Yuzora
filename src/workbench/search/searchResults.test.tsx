import { afterEach, describe, expect, it } from "vitest"
import { fireEvent, render, screen } from "@testing-library/react"

import { SearchResults } from "@/workbench/search/SearchResults"
import { useWorkspaceStore } from "@/state/workspaceStore"

afterEach(() => {
    useWorkspaceStore.setState({
        groups: [{ tabs: [], activePath: null }],
        activeGroupIndex: 0,
        pendingReveal: null
    })
})

describe("SearchResults", () => {
    it("groups matches by file and highlights the hit", () => {
        render(
            <SearchResults
                events={[
                    {
                        type: "match",
                        path: "/w/src/a.ts",
                        matches: [{ line: 3, col: 2, preview: "a needle b" }]
                    },
                    { type: "done", truncated: false, fileCount: 1 }
                ]}
                query="needle"
            />
        )
        expect(screen.getByText("a.ts")).toBeInTheDocument()
        expect(screen.getByText("3")).toBeInTheDocument()
        expect(screen.getByText("needle").tagName).toBe("MARK")
    })

    it("shows truncation notice", () => {
        render(
            <SearchResults
                events={[{ type: "done", truncated: true, fileCount: 5000 }]}
                query="x"
            />
        )
        expect(screen.getByText(/5,000 檔上限/)).toBeInTheDocument()
    })

    it("click match requests reveal", () => {
        render(
            <SearchResults
                events={[
                    {
                        type: "match",
                        path: "/w/a.ts",
                        matches: [{ line: 7, col: 0, preview: "hit" }]
                    },
                    { type: "done", truncated: false, fileCount: 1 }
                ]}
                query="hit"
            />
        )
        fireEvent.click(screen.getByText("hit"))
        // Search clicks reveal-only (focus: false) so the result list keeps focus —
        // navigations (go-to-definition, symbol jump) focus the editor instead (A4).
        expect(useWorkspaceStore.getState().pendingReveal).toEqual({
            path: "/w/a.ts",
            line: 7,
            focus: false
        })
    })
})

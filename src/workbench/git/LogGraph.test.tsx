import { act, cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react"
import { afterEach, expect, it, vi } from "vitest"
import type { LogCommit } from "@/lib/types"
import { LogGraph } from "./LogGraph"

afterEach(cleanup)
function commit(hash: string, parents: string[] = []): LogCommit {
    return { hash, shortHash: hash, subject: `Subject ${hash}`, authorName: "Author", authorEmail: "a@example.com", timestamp: 1770000000, parents, refs: [] }
}
function fixture() {
    return [...Array.from({ length: 32 }, (_, i) => commit(`tip-${i}`, [`parent-${i}`])),
        ...Array.from({ length: 100 }, (_, i) => commit(`middle-${i}`, [`middle-${i + 1}`])),
        ...Array.from({ length: 32 }, (_, i) => commit(`parent-${i}`))]
}
const props = { selectedHash: null, onSelect: vi.fn(), hasMore: false, loadingMore: false, onLoadMore: vi.fn() }

it("sizes graph from all loaded lanes and keeps the width across virtual windows", async () => {
    const commits = fixture()
    render(<LogGraph {...props} commits={commits} />)
    const svg = screen.getByTestId("log-graph-svg")
    const width = Number(svg.getAttribute("width"))
    expect(width).toBeGreaterThanOrEqual(18 + 31 * 16 + 12)
    expect(screen.getAllByRole("button").length).toBeLessThan(commits.length)
    const scroll = screen.getByTestId("log-scroll")
    Object.defineProperty(scroll, "clientHeight", { value: 200, configurable: true })
    scroll.scrollTop = 32 * 80
    fireEvent.scroll(scroll)
    await waitFor(() => expect(screen.queryByText("Subject tip-0")).toBeNull())
    expect(Number(svg.getAttribute("width"))).toBe(width)
    expect(screen.getByTestId("log-header").parentElement).toBe(screen.getByTestId("log-table"))
    expect(screen.getByTestId("log-table").closest('[data-testid="log-scroll"]')).toBe(scroll)
})

it("connects outgoing edges from node centres, incoming edges to centres, and carries unrelated lanes through", () => {
    render(<LogGraph {...props} commits={[
        commit("merge", ["left", "right"]), commit("left", ["base"]),
        commit("right", ["base"]), commit("base"),
    ]} />)
    const groups = [...screen.getByTestId("log-graph-svg").querySelectorAll("g")]
    const paths = groups.map((group) => [...group.querySelectorAll("path")].map((path) => path.getAttribute("d")))
    expect(paths[0]).toContain("M18 16 L18 32")
    expect(paths[0]).toContain("M18 16 C18 24 34 24 34 32")
    expect(paths[1]).toContain("M34 32 L34 64")
    expect(paths[1]).toContain("M18 32 L18 48")
    expect(paths[3]).toContain("M34 96 C34 104 18 104 18 112")
    for (const path of paths[3]) expect(path?.endsWith("18 112")).toBe(true)
})

it("uses the longest offscreen subject and ref chips to size shared columns without truncation", async () => {
    const commits = Array.from({ length: 100 }, (_, i) => commit(`commit-${i}`))
    commits[99].subject = "完整主旨 🚀 ".repeat(100) + "END-OF-SUBJECT"
    commits[99].refs = [{ name: "feature/" + "long-branch-".repeat(30), kind: "local" }]
    render(<LogGraph {...props} commits={commits} />)
    const table = screen.getByTestId("log-table")
    const width = table.style.minWidth
    expect(parseInt(width)).toBeGreaterThan(10000)
    const header = screen.getByTestId("log-header")
    const first = screen.getByRole("button", { name: /Subject commit-0/ })
    expect(first.style.gridTemplateColumns).toBe(header.style.gridTemplateColumns)
    const scroll = screen.getByTestId("log-scroll")
    Object.defineProperty(scroll, "clientHeight", { value: 200, configurable: true })
    scroll.scrollTop = 99 * 32
    fireEvent.scroll(scroll)
    const subject = await screen.findByText(commits[99].subject)
    expect(subject.className).not.toContain("truncate")
    expect(subject.parentElement!.className).not.toContain("overflow-hidden")
    expect(table.style.minWidth).toBe(width)
})

it("ignores horizontal pagination and deduplicates vertical near-bottom events without losing selection", async () => {
    const onLoadMore = vi.fn(), onSelect = vi.fn()
    render(<LogGraph {...props} commits={fixture()} hasMore onLoadMore={onLoadMore} onSelect={onSelect} />)
    const scroll = screen.getByTestId("log-scroll")
    Object.defineProperty(scroll, "scrollHeight", { value: 600, configurable: true })
    Object.defineProperty(scroll, "clientHeight", { value: 500, configurable: true })
    scroll.scrollLeft = 400
    fireEvent.scroll(scroll)
    expect(onLoadMore).not.toHaveBeenCalled()
    scroll.scrollTop = 10
    fireEvent.scroll(scroll)
    expect(onLoadMore).toHaveBeenCalledOnce()
    scroll.scrollTop = 20
    fireEvent.scroll(scroll)
    scroll.scrollLeft = 800
    fireEvent.scroll(scroll)
    expect(onLoadMore).toHaveBeenCalledOnce()
    expect(scroll.scrollTop).toBe(20)
    const row = screen.getByRole("button", { name: /Subject tip-0/ })
    act(() => row.focus())
    fireEvent.keyDown(row, { key: "Enter" })
    fireEvent.keyDown(row, { key: " " })
    expect(onSelect.mock.calls).toEqual([["tip-0"], ["tip-0"]])
    expect(document.activeElement).toBe(row)
})
